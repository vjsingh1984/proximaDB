//! Lock-free Memtable Implementations
//!
//! This module provides lock-free alternatives to existing memtable
//! implementations using DashMap and atomic operations.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

use crate::core::VectorRecord;
use crate::storage::memtable::core::{MemtableConfig, MemtableCore};
use std::collections::HashMap;
use async_trait::async_trait;

/// Lock-free HashMap-based memtable using DashMap
#[derive(Debug)]
pub struct LockFreeHashMapMemtable {
    /// Concurrent hash map for vector storage
    data: Arc<DashMap<String, Arc<VectorRecord>>>,
    /// Current size in bytes (atomic)
    size_bytes: Arc<AtomicUsize>,
    /// Configuration
    config: MemtableConfig,
    /// Metrics
    metrics: Arc<LockFreeMetrics>,
}

/// Lock-free metrics collection
#[derive(Debug)]
pub struct LockFreeMetrics {
    pub read_count: AtomicU64,
    pub write_count: AtomicU64,
    pub delete_count: AtomicU64,
    pub search_count: AtomicU64,
    pub hit_rate: AtomicU64, // Stored as percentage * 100
    pub avg_search_latency_us: AtomicU64,
}

impl LockFreeMetrics {
    pub fn new() -> Self {
        Self {
            read_count: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
            search_count: AtomicU64::new(0),
            hit_rate: AtomicU64::new(0),
            avg_search_latency_us: AtomicU64::new(0),
        }
    }
    
    pub fn record_read(&self) {
        self.read_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_write(&self) {
        self.write_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_delete(&self) {
        self.delete_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_search(&self, latency_us: u64) {
        self.search_count.fetch_add(1, Ordering::Relaxed);
        
        // Update average latency (simplified - in production would use more sophisticated averaging)
        let current_avg = self.avg_search_latency_us.load(Ordering::Relaxed);
        let count = self.search_count.load(Ordering::Relaxed);
        if count > 0 {
            let new_avg = ((current_avg * (count - 1)) + latency_us) / count;
            self.avg_search_latency_us.store(new_avg, Ordering::Relaxed);
        }
    }
}

impl LockFreeHashMapMemtable {
    /// Create new lock-free memtable
    pub fn new(config: MemtableConfig) -> Self {
        Self {
            data: Arc::new(DashMap::new()),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            config,
            metrics: Arc::new(LockFreeMetrics::new()),
        }
    }
    
    /// Estimate size of a vector record in bytes
    fn estimate_record_size(record: &VectorRecord) -> usize {
        let id_size = record.id.as_ref()
            .map(|id| id.len())
            .unwrap_or(0);
        
        let vector_size = record.vector.len() * std::mem::size_of::<f32>();
        let metadata_size = record.metadata.len() * 64; // Rough estimate
        
        id_size + vector_size + metadata_size + 64 // 64 bytes overhead
    }
    
    /// Insert a vector record
    pub async fn insert_vector(&self, record: VectorRecord) -> Result<()> {
        let id = record.id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Vector ID required"))?
            .clone();
        
        let record_size = Self::estimate_record_size(&record);
        let record_arc = Arc::new(record);
        
        // Check size limit
        let current_size = self.size_bytes.load(Ordering::Relaxed);
        if current_size + record_size > self.config.max_size_bytes {
            return Err(anyhow::anyhow!("Memtable size limit exceeded"));
        }
        
        // Insert or update
        match self.data.insert(id, record_arc) {
            Some(old_record) => {
                // Update size (subtract old, add new)
                let old_size = Self::estimate_record_size(&old_record);
                self.size_bytes.fetch_sub(old_size, Ordering::Relaxed);
                self.size_bytes.fetch_add(record_size, Ordering::Relaxed);
            }
            None => {
                // New insertion
                self.size_bytes.fetch_add(record_size, Ordering::Relaxed);
            }
        }
        
        self.metrics.record_write();
        Ok(())
    }
    
    /// Get a vector record by ID
    pub async fn get_vector(&self, id: &str) -> Result<Option<VectorRecord>> {
        self.metrics.record_read();
        
        let result = self.data
            .get(id)
            .map(|entry| (**entry.value()).clone());
        Ok(result)
    }
    
    /// Delete a vector record
    pub async fn delete_vector(&self, id: &str) -> Result<bool> {
        self.metrics.record_delete();
        
        if let Some((_, record)) = self.data.remove(id) {
            let size = Self::estimate_record_size(&record);
            self.size_bytes.fetch_sub(size, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    /// Get all records (for serialization)
    pub async fn get_all_records(&self) -> Result<Vec<VectorRecord>> {
        let records: Vec<VectorRecord> = self.data
            .iter()
            .map(|entry| (**entry.value()).clone())
            .collect();
        Ok(records)
    }
    
    /// Get metrics
    pub fn metrics(&self) -> HashMap<String, f64> {
        let mut metrics = HashMap::new();
        
        metrics.insert("read_count".to_string(), 
            self.metrics.read_count.load(Ordering::Relaxed) as f64);
        metrics.insert("write_count".to_string(), 
            self.metrics.write_count.load(Ordering::Relaxed) as f64);
        metrics.insert("delete_count".to_string(), 
            self.metrics.delete_count.load(Ordering::Relaxed) as f64);
        metrics.insert("search_count".to_string(), 
            self.metrics.search_count.load(Ordering::Relaxed) as f64);
        metrics.insert("hit_rate".to_string(), 
            self.metrics.hit_rate.load(Ordering::Relaxed) as f64 / 100.0);
        metrics.insert("avg_search_latency_us".to_string(), 
            self.metrics.avg_search_latency_us.load(Ordering::Relaxed) as f64);
        metrics.insert("size_bytes".to_string(), 
            self.size_bytes.load(Ordering::Relaxed) as f64);
        metrics.insert("record_count".to_string(), 
            self.data.len() as f64);
        
        metrics
    }
}

// Implement MemtableCore trait for generic key-value storage
#[async_trait]
impl<K, V> MemtableCore<K, V> for LockFreeHashMapMemtable
where
    K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + 'static,
    V: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    async fn insert(&self, _key: K, _value: V) -> Result<u64> {
        // For now, just store in a generic way - would need proper serialization
        // This is a placeholder implementation
        Ok(64) // Return estimated size
    }

    async fn get(&self, _key: &K) -> Result<Option<V>> {
        // Placeholder implementation
        Ok(None)
    }

    async fn range_scan(&self, _from: K, _limit: Option<usize>) -> Result<Vec<(K, V)>> {
        Ok(vec![])
    }

    async fn size_bytes(&self) -> usize {
        self.size_bytes.load(Ordering::Relaxed)
    }

    async fn len(&self) -> usize {
        self.data.len()
    }

    async fn clear_up_to(&self, _threshold: K) -> Result<usize> {
        Ok(0)
    }

    async fn clear(&self) -> Result<()> {
        self.data.clear();
        self.size_bytes.store(0, Ordering::Relaxed);
        Ok(())
    }

    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_lockfree_hashmap_basic_operations() {
        let config = MemtableConfig::default();
        let memtable = LockFreeHashMapMemtable::new(config);
        
        // Test insert
        let record = VectorRecord {
            id: Some("test_id".to_string()),
            vector: vec![0.1, 0.2, 0.3],
            metadata: vec![],
            ..Default::default()
        };
        
        memtable.insert_vector(record.clone()).await.unwrap();
        
        // Test get
        let retrieved = memtable.get_vector("test_id").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, record.id);
        
        // Test delete
        let deleted = memtable.delete_vector("test_id").await.unwrap();
        assert!(deleted);
        
        // Verify deletion
        let after_delete = memtable.get_vector("test_id").await.unwrap();
        assert!(after_delete.is_none());
    }
    
    #[tokio::test]
    async fn test_concurrent_operations() {
        let config = MemtableConfig::default();
        let memtable = Arc::new(LockFreeHashMapMemtable::new(config));
        
        let mut handles = vec![];
        
        // Spawn 100 concurrent inserts
        for i in 0..100 {
            let memtable_clone = memtable.clone();
            let handle = tokio::spawn(async move {
                let record = VectorRecord {
                    id: Some(format!("id_{}", i)),
                    vector: vec![i as f32; 128],
                    metadata: vec![],
                    ..Default::default()
                };
                
                memtable_clone.insert_vector(record).await
            });
            handles.push(handle);
        }
        
        // Wait for all insertions
        for handle in handles {
            handle.await.unwrap().unwrap();
        }
        
        // Verify all records exist
        for i in 0..100 {
            let record = memtable.get_vector(&format!("id_{}", i)).await.unwrap();
            assert!(record.is_some());
        }
        
        // Check metrics
        let metrics = memtable.metrics();
        assert_eq!(metrics["write_count"], 100.0);
        assert_eq!(metrics["record_count"], 100.0);
    }
}