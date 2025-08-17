// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AXIS Index Memory Tracker - Tracks index residency and manages eviction fallback
//! 
//! This module provides memory residency tracking for AXIS indexes, enabling
//! intelligent fallback to raw storage when indexes are evicted.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Tracks memory status of indexes for each collection
#[derive(Debug, Clone)]
pub struct IndexMemoryTracker {
    /// Collection ID -> Index memory status
    collection_status: Arc<DashMap<String, IndexMemoryStatus>>,
    
    /// Total memory used by indexes
    total_memory_bytes: Arc<AtomicUsize>,
    
    /// Maximum memory allowed for indexes
    max_memory_bytes: usize,
    
    /// LRU eviction queue (collection_id, last_access_time)
    eviction_queue: Arc<RwLock<Vec<(String, std::time::Instant)>>>,
}

/// Memory status for a collection's indexes
#[derive(Debug, Clone)]
pub struct IndexMemoryStatus {
    pub collection_id: String,
    pub index_type: IndexType,
    pub memory_state: MemoryState,
    pub memory_bytes: usize,
    pub last_access: std::time::Instant,
    pub access_count: u64,
    pub fallback_count: u64,
    pub disk_location: Option<String>,
}

/// Types of indexes
#[derive(Debug, Clone, PartialEq)]
pub enum IndexType {
    HNSW { layers: usize },
    IVF { centroids: usize },
    PQ { codebooks: usize },
    Flat,
    LSH { tables: usize },
    Annoy { trees: usize },
}

/// Memory residency state
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryState {
    /// Fully loaded in memory
    InMemory,
    
    /// Partially loaded (e.g., only top HNSW layers or IVF centroids)
    PartiallyLoaded {
        memory_percentage: f32,
        loaded_components: Vec<String>,
    },
    
    /// Evicted to disk
    Evicted {
        eviction_time: std::time::Instant,
        reason: EvictionReason,
    },
    
    /// Not yet built
    NotBuilt,
    
    /// Being loaded from disk
    Loading {
        progress_percentage: f32,
    },
}

/// Reasons for eviction
#[derive(Debug, Clone, PartialEq)]
pub enum EvictionReason {
    MemoryPressure,
    LowAccessFrequency,
    ManualEviction,
    CollectionDeleted,
}

impl IndexMemoryTracker {
    /// Create a new memory tracker
    pub fn new(max_memory_gb: f32) -> Self {
        Self {
            collection_status: Arc::new(DashMap::new()),
            total_memory_bytes: Arc::new(AtomicUsize::new(0)),
            max_memory_bytes: (max_memory_gb * 1024.0 * 1024.0 * 1024.0) as usize,
            eviction_queue: Arc::new(RwLock::new(Vec::new())),
        }
    }
    
    /// Check if an index is available in memory
    pub async fn is_index_available(&self, collection_id: &str) -> bool {
        if let Some(status) = self.collection_status.get(key) {
            matches!(
                status.memory_state,
                MemoryState::InMemory | MemoryState::PartiallyLoaded { .. }
            )
        } else {
            false
        }
    }
    
    /// Get detailed memory status for a collection
    pub async fn get_memory_status(&self, collection_id: &str) -> Option<IndexMemoryStatus> {
        self.collection_status.get(key).map(|s| s.clone())
    }
    
    /// Register an index as loaded in memory
    pub async fn register_index_loaded(
        &self,
        collection_id: String,
        index_type: IndexType,
        memory_bytes: usize,
    ) -> Result<()> {
        // Check if we need to evict something first
        let current_total = self.total_memory_bytes.load(Ordering::Relaxed);
        if current_total + memory_bytes > self.max_memory_bytes {
            self.evict_lru_indexes(memory_bytes).await?;
        }
        
        let status = IndexMemoryStatus {
            collection_id: collection_id.clone(),
            index_type,
            memory_state: MemoryState::InMemory,
            memory_bytes,
            last_access: std::time::Instant::now(),
            access_count: 0,
            fallback_count: 0,
            disk_location: None,
        };
        
        self.collection_status.insert(collection_id.clone(), status);
        self.total_memory_bytes.fetch_add(memory_bytes, Ordering::Relaxed);
        
        info!("✅ AXIS: Index loaded for collection {} ({:.2} MB)", 
              collection_id, memory_bytes as f64 / 1_048_576.0);
        
        Ok(())
    }
    
    /// Mark an index as evicted
    pub async fn mark_evicted(
        &self,
        collection_id: &str,
        reason: EvictionReason,
        disk_location: Option<String>,
    ) -> Result<()> {
        if let Some(mut status) = self.collection_status.get_mut(collection_id) {
            let memory_freed = status.memory_bytes;
            
            status.memory_state = MemoryState::Evicted {
                eviction_time: std::time::Instant::now(),
                reason: reason.clone(),
            };
            status.disk_location = disk_location;
            status.memory_bytes = 0;
            
            self.total_memory_bytes.fetch_sub(memory_freed, Ordering::Relaxed);
            
            warn!("⚠️ AXIS: Index evicted for collection {} (reason: {:?}, freed: {:.2} MB)",
                  collection_id, reason, memory_freed as f64 / 1_048_576.0);
        }
        
        Ok(())
    }
    
    /// Record an index access
    pub async fn record_access(&self, collection_id: &str) {
        if let Some(mut status) = self.collection_status.get_mut(collection_id) {
            status.last_access = std::time::Instant::now();
            status.access_count += 1;
        }
    }
    
    /// Record a fallback to storage
    pub async fn record_fallback(&self, collection_id: &str) {
        if let Some(mut status) = self.collection_status.get_mut(collection_id) {
            status.fallback_count += 1;
        }
        
        debug!("📊 AXIS: Fallback to storage for collection {}", collection_id);
    }
    
    /// Evict least recently used indexes to free memory
    async fn evict_lru_indexes(&self, required_bytes: usize) -> Result<()> {
        let mut eviction_candidates: Vec<(String, std::time::Instant, usize)> = Vec::new();
        
        // Collect candidates sorted by last access time
        for entry in self.collection_status.iter() {
            if matches!(entry.memory_state, MemoryState::InMemory) {
                eviction_candidates.push((
                    entry.collection_id.clone(),
                    entry.last_access,
                    entry.memory_bytes,
                ));
            }
        }
        
        // Sort by last access time (oldest first)
        eviction_candidates.sort_by_key(|(_, last_access, _)| *last_access);
        
        let mut freed_bytes = 0;
        for (collection_id, _, memory_bytes) in eviction_candidates {
            if freed_bytes >= required_bytes {
                break;
            }
            
            self.mark_evicted(&collection_id, EvictionReason::MemoryPressure, None).await?;
            freed_bytes += memory_bytes;
        }
        
        Ok(())
    }
    
    /// Get memory usage statistics
    pub async fn get_memory_stats(&self) -> MemoryStats {
        let total_used = self.total_memory_bytes.load(Ordering::Relaxed);
        let mut in_memory_count = 0;
        let mut partial_count = 0;
        let mut evicted_count = 0;
        let mut total_fallbacks = 0;
        
        for entry in self.collection_status.iter() {
            match entry.memory_state {
                MemoryState::InMemory => in_memory_count += 1,
                MemoryState::PartiallyLoaded { .. } => partial_count += 1,
                MemoryState::Evicted { .. } => evicted_count += 1,
                _ => {}
            }
            total_fallbacks += entry.fallback_count;
        }
        
        MemoryStats {
            total_memory_bytes: total_used,
            max_memory_bytes: self.max_memory_bytes,
            memory_usage_percentage: (total_used as f64 / self.max_memory_bytes as f64) * 100.0,
            collections_in_memory: in_memory_count,
            collections_partial: partial_count,
            collections_evicted: evicted_count,
            total_fallback_count: total_fallbacks,
        }
    }
    
    /// Load index from disk (async operation)
    pub async fn load_index_from_disk(
        &self,
        collection_id: &str,
    ) -> Result<bool> {
        // First check if disk location exists
        let disk_location = {
            if let Some(status) = self.collection_status.get(key) {
                status.disk_location.clone()
            } else {
                return Ok(false);
            }
        };
        
        if let Some(location) = disk_location {
            // Now update the state
            if let Some(mut status) = self.collection_status.get_mut(collection_id) {
                status.memory_state = MemoryState::Loading {
                    progress_percentage: 0.0,
                };
            }
            
            info!("📥 AXIS: Loading index from disk for collection {} ({})",
                  collection_id, location);
            
            // TODO: Actual loading implementation would go here
            // This would involve reading the serialized index from disk
            // and reconstructing it in memory
            
            Ok(true)
        } else {
            Ok(false) // No disk location available
        }
    }
}

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_memory_bytes: usize,
    pub max_memory_bytes: usize,
    pub memory_usage_percentage: f64,
    pub collections_in_memory: usize,
    pub collections_partial: usize,
    pub collections_evicted: usize,
    pub total_fallback_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_memory_tracking() {
        let tracker = IndexMemoryTracker::new(1.0); // 1 GB max
        
        // Register an index
        tracker.register_index_loaded(
            "collection1".to_string(),
            IndexType::HNSW { layers: 4 },
            500_000_000, // 500 MB
        ).await.unwrap();
        
        assert!(tracker.is_index_available("collection1").await);
        
        // Register another index that triggers eviction
        tracker.register_index_loaded(
            "collection2".to_string(),
            IndexType::IVF { centroids: 1024 },
            600_000_000, // 600 MB
        ).await.unwrap();
        
        // First collection should be evicted
        assert!(!tracker.is_index_available("collection1").await);
        assert!(tracker.is_index_available("collection2").await);
        
        let stats = tracker.get_memory_stats().await;
        assert_eq!(stats.collections_evicted, 1);
        assert_eq!(stats.collections_in_memory, 1);
    }
    
    #[tokio::test]
    async fn test_fallback_tracking() {
        let tracker = IndexMemoryTracker::new(1.0);
        
        tracker.register_index_loaded(
            "collection1".to_string(),
            IndexType::Flat,
            100_000_000,
        ).await.unwrap();
        
        tracker.record_access("collection1").await;
        tracker.record_access("collection1").await;
        tracker.record_fallback("collection1").await;
        
        let status = tracker.get_memory_status("collection1").await.unwrap();
        assert_eq!(status.access_count, 2);
        assert_eq!(status.fallback_count, 1);
    }
}