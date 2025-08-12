/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Unified concurrent data structures for both cache and index systems
//!
//! This module provides reusable, high-performance concurrent data structures
//! that are shared across cache systems and index implementations:
//!
//! - ConcurrentStorage: Generic DashMap-based storage with metrics
//! - AtomicMetrics: Performance and usage tracking  
//! - ConcurrentMapping: Bidirectional key mapping
//! - TypedStorage: Type-safe storage with validation

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Generic concurrent storage with automatic metrics tracking
/// Used by both cache systems and index implementations
pub struct ConcurrentStorage<K, V> 
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Main storage using DashMap for lock-free operations
    storage: DashMap<K, StoredItem<V>>,
    
    /// Automatic metrics tracking
    metrics: AtomicMetrics,
    
    /// Optional capacity limit (0 = unlimited)
    max_capacity: usize,
    
    /// Optional memory limit in bytes (0 = unlimited) 
    max_memory_bytes: usize,
    
    /// Optional custom memory estimator (not debuggable)
    memory_estimator: Option<Box<dyn Fn(&K, &V) -> usize + Send + Sync>>,
}

// Manual Debug implementation to handle non-debuggable function
impl<K, V> std::fmt::Debug for ConcurrentStorage<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static + std::fmt::Debug,
    V: Clone + Send + Sync + 'static + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConcurrentStorage")
            .field("storage", &self.storage)
            .field("metrics", &self.metrics)
            .field("max_capacity", &self.max_capacity)
            .field("max_memory_bytes", &self.max_memory_bytes)
            .field("memory_estimator", &self.memory_estimator.is_some())
            .finish()
    }
}

/// Wrapper for stored items with metadata
struct StoredItem<V> {
    value: V,
    created_at: Instant,
    last_accessed: Instant,
    access_count: AtomicUsize,
    size_bytes: usize,
}

// Manual implementations since AtomicUsize doesn't implement Clone/Debug trivially
impl<V: Clone> Clone for StoredItem<V> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            created_at: self.created_at,
            last_accessed: self.last_accessed,
            access_count: AtomicUsize::new(self.access_count.load(Ordering::Relaxed)),
            size_bytes: self.size_bytes,
        }
    }
}

impl<V: std::fmt::Debug> std::fmt::Debug for StoredItem<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredItem")
            .field("value", &self.value)
            .field("created_at", &self.created_at)
            .field("last_accessed", &self.last_accessed)
            .field("access_count", &self.access_count.load(Ordering::Relaxed))
            .field("size_bytes", &self.size_bytes)
            .finish()
    }
}

impl<K, V> ConcurrentStorage<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Create new concurrent storage
    pub fn new() -> Self {
        Self {
            storage: DashMap::new(),
            metrics: AtomicMetrics::new(),
            max_capacity: 0,
            max_memory_bytes: 0,
            memory_estimator: None,
        }
    }

    /// Create with capacity limit
    pub fn with_capacity(max_capacity: usize) -> Self {
        Self {
            storage: DashMap::with_capacity(max_capacity.min(1024)), // Pre-allocate reasonably
            metrics: AtomicMetrics::new(),
            max_capacity,
            max_memory_bytes: 0,
            memory_estimator: None,
        }
    }

    /// Create with memory limit
    pub fn with_memory_limit(max_memory_bytes: usize) -> Self {
        Self {
            storage: DashMap::new(),
            metrics: AtomicMetrics::new(),
            max_capacity: 0,
            max_memory_bytes,
            memory_estimator: None,
        }
    }

    /// Set custom memory estimator
    pub fn with_memory_estimator<F>(mut self, estimator: F) -> Self 
    where
        F: Fn(&K, &V) -> usize + Send + Sync + 'static,
    {
        self.memory_estimator = Some(Box::new(estimator));
        self
    }

    /// Insert item with automatic metrics
    pub fn insert(&self, key: K, value: V) -> Result<Option<V>> {
        let start = Instant::now();

        // Check capacity limits
        if self.max_capacity > 0 && self.storage.len() >= self.max_capacity {
            self.metrics.record_failure(start.elapsed());
            return Err(anyhow!("Storage capacity exceeded"));
        }

        // Estimate memory usage
        let size_bytes = self.estimate_memory(&key, &value);
        if self.max_memory_bytes > 0 {
            let current_memory = self.metrics.memory_bytes();
            if current_memory + size_bytes > self.max_memory_bytes {
                self.metrics.record_failure(start.elapsed());
                return Err(anyhow!("Memory limit exceeded"));
            }
        }

        let now = Instant::now();
        let item = StoredItem {
            value: value.clone(),
            created_at: now,
            last_accessed: now,
            access_count: AtomicUsize::new(1),
            size_bytes,
        };

        let old_value = self.storage.insert(key, item).map(|old| {
            // Update metrics for replacement
            self.metrics.add_memory_bytes(-(old.size_bytes as i64));
            old.value
        });

        // Update metrics for new item
        if old_value.is_none() {
            self.metrics.increment_entries();
        }
        self.metrics.add_memory_bytes(size_bytes as i64);
        self.metrics.record_success(start.elapsed());

        Ok(old_value)
    }

    /// Get item with access tracking
    pub fn get(&self, key: &K) -> Option<V> {
        let start = Instant::now();
        
        if let Some(mut entry) = self.storage.get_mut(key) {
            // Update access metadata
            entry.last_accessed = Instant::now();
            entry.access_count.fetch_add(1, Ordering::Relaxed);
            
            let value = entry.value.clone();
            
            self.metrics.record_hit();
            self.metrics.record_success(start.elapsed());
            
            Some(value)
        } else {
            self.metrics.record_miss();
            self.metrics.record_success(start.elapsed());
            None
        }
    }

    /// Remove item
    pub fn remove(&self, key: &K) -> Option<V> {
        let start = Instant::now();
        
        if let Some((_, item)) = self.storage.remove(key) {
            self.metrics.decrement_entries();
            self.metrics.add_memory_bytes(-(item.size_bytes as i64));
            self.metrics.record_success(start.elapsed());
            Some(item.value)
        } else {
            self.metrics.record_success(start.elapsed());
            None
        }
    }

    /// Check if key exists
    pub fn contains(&self, key: &K) -> bool {
        self.storage.contains_key(key)
    }

    /// Get current length
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<K> {
        self.storage.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Get metrics snapshot
    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Clear all entries
    pub fn clear(&self) {
        let count = self.storage.len();
        let memory = self.metrics.memory_bytes();
        
        self.storage.clear();
        self.metrics.add_entries(-(count as i64));
        self.metrics.add_memory_bytes(-(memory as i64));
    }

    /// Get access metadata for a key
    pub fn access_info(&self, key: &K) -> Option<AccessInfo> {
        self.storage.get(key).map(|entry| AccessInfo {
            created_at: entry.created_at,
            last_accessed: entry.last_accessed,
            access_count: entry.access_count.load(Ordering::Relaxed),
            size_bytes: entry.size_bytes,
        })
    }

    /// Estimate memory usage for key-value pair
    fn estimate_memory(&self, key: &K, value: &V) -> usize {
        if let Some(ref estimator) = self.memory_estimator {
            estimator(key, value)
        } else {
            // Default estimation
            std::mem::size_of::<K>() + std::mem::size_of::<V>() + 64 // DashMap overhead
        }
    }
}

/// Access information for cache analysis
#[derive(Debug, Clone)]
pub struct AccessInfo {
    pub created_at: Instant,
    pub last_accessed: Instant,
    pub access_count: usize,
    pub size_bytes: usize,
}

/// Atomic metrics for concurrent operations
#[derive(Debug)]
pub struct AtomicMetrics {
    /// Total number of entries
    entries: AtomicUsize,
    /// Total memory usage in bytes
    memory_bytes: AtomicUsize,
    /// Total operations performed
    operations: AtomicU64,
    /// Cache hits
    hits: AtomicU64,
    /// Cache misses
    misses: AtomicU64,
    /// Successful operations
    successful: AtomicU64,
    /// Failed operations
    failed: AtomicU64,
    /// Total operation time in nanoseconds
    total_time_ns: AtomicU64,
}

impl AtomicMetrics {
    pub fn new() -> Self {
        Self {
            entries: AtomicUsize::new(0),
            memory_bytes: AtomicUsize::new(0),
            operations: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            successful: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            total_time_ns: AtomicU64::new(0),
        }
    }

    pub fn increment_entries(&self) {
        self.entries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_entries(&self) {
        self.entries.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn add_entries(&self, delta: i64) {
        if delta >= 0 {
            self.entries.fetch_add(delta as usize, Ordering::Relaxed);
        } else {
            self.entries.fetch_sub((-delta) as usize, Ordering::Relaxed);
        }
    }

    pub fn add_memory_bytes(&self, delta: i64) {
        if delta >= 0 {
            self.memory_bytes.fetch_add(delta as usize, Ordering::Relaxed);
        } else {
            self.memory_bytes.fetch_sub((-delta) as usize, Ordering::Relaxed);
        }
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        self.operations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.operations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self, duration: Duration) {
        self.successful.fetch_add(1, Ordering::Relaxed);
        self.total_time_ns.fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn record_failure(&self, duration: Duration) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        self.total_time_ns.fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn entries(&self) -> usize {
        self.entries.load(Ordering::Relaxed)
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes.load(Ordering::Relaxed)
    }

    pub fn hit_rate(&self) -> f64 {
        let total = self.operations.load(Ordering::Relaxed);
        if total == 0 { return 0.0; }
        self.hits.load(Ordering::Relaxed) as f64 / total as f64
    }

    pub fn record_operation(&self, _op_name: &str, duration: Duration) {
        self.operations.fetch_add(1, Ordering::Relaxed);
        self.successful.fetch_add(1, Ordering::Relaxed);
        self.total_time_ns.fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn avg_operation_time(&self) -> Duration {
        let total_ops = self.successful.load(Ordering::Relaxed) + self.failed.load(Ordering::Relaxed);
        if total_ops == 0 { return Duration::ZERO; }
        
        let avg_ns = self.total_time_ns.load(Ordering::Relaxed) / total_ops;
        Duration::from_nanos(avg_ns)
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            entries: self.entries(),
            memory_bytes: self.memory_bytes(),
            operations: self.operations.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            hit_rate: self.hit_rate(),
            avg_operation_time: self.avg_operation_time(),
        }
    }
}

impl Default for AtomicMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of metrics at a point in time
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct MetricsSnapshot {
    pub entries: usize,
    pub memory_bytes: usize,
    pub operations: u64,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub avg_operation_time: Duration,
}

/// Bidirectional mapping between two key types
/// Used by indexes for internal/external ID mapping
#[derive(Debug)]
pub struct ConcurrentMapping<K1, K2>
where
    K1: Hash + Eq + Clone + Send + Sync + 'static,
    K2: Hash + Eq + Clone + Send + Sync + 'static,
{
    forward: DashMap<K1, K2>,
    reverse: DashMap<K2, K1>,
    metrics: AtomicMetrics,
}

impl<K1, K2> ConcurrentMapping<K1, K2>
where
    K1: Hash + Eq + Clone + Send + Sync + 'static,
    K2: Hash + Eq + Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            forward: DashMap::new(),
            reverse: DashMap::new(),
            metrics: AtomicMetrics::new(),
        }
    }

    /// Insert bidirectional mapping
    pub fn insert(&self, key1: K1, key2: K2) -> Result<()> {
        let start = Instant::now();

        // Check for existing mappings
        if self.forward.contains_key(&key1) || self.reverse.contains_key(&key2) {
            self.metrics.record_failure(start.elapsed());
            return Err(anyhow!("Key already exists in mapping"));
        }

        self.forward.insert(key1.clone(), key2.clone());
        self.reverse.insert(key2, key1);
        
        self.metrics.increment_entries();
        self.metrics.record_success(start.elapsed());
        
        Ok(())
    }

    /// Get K2 from K1
    pub fn get_forward(&self, key1: &K1) -> Option<K2> {
        self.forward.get(key1).map(|entry| entry.value().clone())
    }

    /// Get K1 from K2
    pub fn get_reverse(&self, key2: &K2) -> Option<K1> {
        self.reverse.get(key2).map(|entry| entry.value().clone())
    }

    /// Remove by K1
    pub fn remove_forward(&self, key1: &K1) -> Option<K2> {
        if let Some((_, key2)) = self.forward.remove(key1) {
            self.reverse.remove(&key2);
            self.metrics.decrement_entries();
            Some(key2)
        } else {
            None
        }
    }

    /// Remove by K2
    pub fn remove_reverse(&self, key2: &K2) -> Option<K1> {
        if let Some((_, key1)) = self.reverse.remove(key2) {
            self.forward.remove(&key1);
            self.metrics.decrement_entries();
            Some(key1)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.forward.len()
    }

    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }
}

impl<K1, K2> Default for ConcurrentMapping<K1, K2>
where
    K1: Hash + Eq + Clone + Send + Sync + 'static,
    K2: Hash + Eq + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Type-safe storage with serialization support
/// Useful for cache systems that need to serialize/deserialize values
#[derive(Debug)]
pub struct TypedStorage<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    inner: ConcurrentStorage<K, V>,
}

impl<K, V> TypedStorage<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    pub fn new() -> Self {
        Self {
            inner: ConcurrentStorage::new(),
        }
    }

    pub fn with_capacity(max_capacity: usize) -> Self {
        Self {
            inner: ConcurrentStorage::with_capacity(max_capacity),
        }
    }

    /// All methods delegate to inner storage
    pub fn insert(&self, key: K, value: V) -> Result<Option<V>> {
        self.inner.insert(key, value)
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.inner.get(key)
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.inner.contains(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.inner.metrics()
    }

    /// Serialize value to bytes (for persistence)
    pub fn serialize_value(&self, value: &V) -> Result<Vec<u8>> {
        bincode::serialize(value).map_err(|e| anyhow!("Serialization failed: {}", e))
    }

    /// Deserialize value from bytes
    pub fn deserialize_value(&self, bytes: &[u8]) -> Result<V> {
        bincode::deserialize(bytes).map_err(|e| anyhow!("Deserialization failed: {}", e))
    }
}

impl<K, V> Default for TypedStorage<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concurrent_storage() {
        let storage = ConcurrentStorage::new();
        
        // Test insert
        assert!(storage.insert("key1", "value1").unwrap().is_none());
        assert_eq!(storage.len(), 1);
        
        // Test get with metrics
        assert_eq!(storage.get(&"key1"), Some("value1"));
        let metrics = storage.metrics();
        assert_eq!(metrics.hits, 1);
        
        // Test miss
        assert_eq!(storage.get(&"nonexistent"), None);
        let metrics = storage.metrics();
        assert_eq!(metrics.misses, 1);
        
        // Test remove
        assert_eq!(storage.remove(&"key1"), Some("value1"));
        assert_eq!(storage.len(), 0);
    }

    #[test]
    fn test_concurrent_mapping() {
        let mapping = ConcurrentMapping::new();
        
        // Test insert
        mapping.insert("external1", 1usize).unwrap();
        mapping.insert("external2", 2usize).unwrap();
        
        // Test forward lookup
        assert_eq!(mapping.get_forward(&"external1"), Some(1));
        
        // Test reverse lookup
        assert_eq!(mapping.get_reverse(&2), Some("external2"));
        
        // Test remove
        assert_eq!(mapping.remove_forward(&"external1"), Some(1));
        assert_eq!(mapping.len(), 1);
    }

    #[test]
    fn test_storage_with_limits() {
        let storage = ConcurrentStorage::with_capacity(2);
        
        // Insert up to capacity
        assert!(storage.insert(1, "value1").is_ok());
        assert!(storage.insert(2, "value2").is_ok());
        
        // Exceed capacity
        assert!(storage.insert(3, "value3").is_err());
    }
}