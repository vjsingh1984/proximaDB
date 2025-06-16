// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL Age Monitoring and Flush Triggering
//! 
//! Monitors collection WAL ages and triggers flushes when thresholds are exceeded.

use anyhow::Result;
use chrono::{DateTime, Utc, Duration};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{interval, Duration as TokioDuration};

use crate::core::CollectionId;
use super::WalConfig;
use super::memtable::{MemTableStrategy, FlushIsolationCoordinator, IsolationReason};

/// WAL age monitoring and flush triggering service
pub struct WalAgeMonitor {
    /// WAL manager reference
    wal_manager: Arc<dyn WalManagerTrait>,
    
    /// Flush isolation coordinator
    flush_coordinator: Arc<FlushIsolationCoordinator>,
    
    /// Configuration
    config: Arc<WalConfig>,
    
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
    
    /// Monitoring statistics
    stats: Arc<tokio::sync::RwLock<AgeMonitorStats>>,
}

/// Statistics for age monitoring
#[derive(Debug, Clone, Default)]
pub struct AgeMonitorStats {
    /// Total age checks performed
    pub total_checks: u64,
    
    /// Total flushes triggered by age
    pub age_triggered_flushes: u64,
    
    /// Collections currently being monitored
    pub monitored_collections: usize,
    
    /// Last check timestamp
    pub last_check_at: Option<DateTime<Utc>>,
    
    /// Average age check duration
    pub avg_check_duration_ms: f64,
    
    /// Oldest collection ages found
    pub oldest_collection_ages: HashMap<CollectionId, Duration>,
}

/// Collection age information
#[derive(Debug, Clone)]
pub struct CollectionAgeInfo {
    pub collection_id: CollectionId,
    pub oldest_entry_timestamp: DateTime<Utc>,
    pub age: Duration,
    pub threshold: Duration,
    pub needs_flush: bool,
    pub entry_count: u64,
    pub memory_usage: usize,
}

impl WalAgeMonitor {
    /// Create new WAL age monitor
    pub fn new(
        wal_manager: Arc<dyn WalManagerTrait>,
        flush_coordinator: Arc<FlushIsolationCoordinator>,
        config: Arc<WalConfig>,
    ) -> Self {
        Self {
            wal_manager,
            flush_coordinator,
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(tokio::sync::RwLock::new(AgeMonitorStats::default())),
        }
    }
    
    /// Start the age monitoring background task
    pub async fn start(&self) -> Result<()> {
        let check_interval = self.config.performance.age_check_interval_secs;
        let mut interval = interval(TokioDuration::from_secs(check_interval));
        
        tracing::info!("🕐 Starting WAL age monitor with {} second intervals", check_interval);
        
        while !self.shutdown.load(Ordering::Relaxed) {
            interval.tick().await;
            
            if let Err(e) = self.perform_age_check().await {
                tracing::error!("❌ WAL age check failed: {}", e);
            }
        }
        
        tracing::info!("🛑 WAL age monitor stopped");
        Ok(())
    }
    
    /// Stop the age monitoring
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
    
    /// Perform a single age check cycle
    async fn perform_age_check(&self) -> Result<()> {
        let start_time = Utc::now();
        
        tracing::debug!("🕐 Starting WAL age check");
        
        // Get memtable strategy from WAL manager
        let memtable = self.wal_manager.get_memtable_strategy().await?;
        
        // Get collection ages
        let collection_ages = memtable.get_collection_ages().await?;
        let total_collections = collection_ages.len();
        
        let mut flush_candidates = Vec::new();
        let mut oldest_ages = HashMap::new();
        
        // Check each collection against its threshold
        for (collection_id, age) in collection_ages {
            let threshold_secs = self.config.effective_max_age_for_collection(&collection_id);
            let threshold = Duration::seconds(threshold_secs as i64);
            
            oldest_ages.insert(collection_id.clone(), age);
            
            if age > threshold {
                // Get additional info for flush decision
                let oldest_timestamp = memtable.get_oldest_unflushed_timestamp(&collection_id).await?;
                let stats = memtable.get_stats().await?;
                let collection_stats = stats.get(&collection_id);
                
                let age_info = CollectionAgeInfo {
                    collection_id: collection_id.clone(),
                    oldest_entry_timestamp: oldest_timestamp.unwrap_or_else(Utc::now),
                    age,
                    threshold,
                    needs_flush: true,
                    entry_count: collection_stats.map(|s| s.total_entries).unwrap_or(0),
                    memory_usage: collection_stats.map(|s| s.memory_bytes).unwrap_or(0),
                };
                
                flush_candidates.push(age_info);
            }
        }
        
        // Trigger flushes for aged collections
        let triggered_flushes = flush_candidates.len();
        for age_info in flush_candidates {
            if let Err(e) = self.trigger_age_based_flush(&age_info).await {
                tracing::error!("❌ Failed to trigger age-based flush for collection {}: {}", 
                              age_info.collection_id, e);
            }
        }
        
        // Update statistics
        let check_duration = Utc::now().signed_duration_since(start_time);
        self.update_stats(total_collections, triggered_flushes, check_duration.num_milliseconds() as f64, oldest_ages).await;
        
        tracing::debug!("✅ WAL age check completed in {}ms, checked {} collections, triggered {} flushes", 
                       check_duration.num_milliseconds(), total_collections, triggered_flushes);
        
        Ok(())
    }
    
    /// Trigger flush for an aged collection
    async fn trigger_age_based_flush(&self, age_info: &CollectionAgeInfo) -> Result<()> {
        tracing::info!("⏰ Triggering age-based flush for collection {} (age: {}m, threshold: {}m, entries: {}, memory: {}MB)", 
                      age_info.collection_id,
                      age_info.age.num_minutes(),
                      age_info.threshold.num_minutes(),
                      age_info.entry_count,
                      age_info.memory_usage / (1024 * 1024));
        
        // Check if collection is already being flushed
        if self.flush_coordinator.is_isolated(&age_info.collection_id).await {
            tracing::debug!("⏸️ Collection {} already isolated, skipping age-based flush", age_info.collection_id);
            return Ok(());
        }
        
        // Start isolation for flush
        let memtable = self.wal_manager.get_memtable_strategy().await?;
        let isolatable_memtable = memtable.as_any()
            .downcast_ref::<super::memtable::skiplist_isolatable::IsolatableSkipListMemTable>()
            .ok_or_else(|| anyhow::anyhow!("Memtable does not support isolation"))?;
        
        self.flush_coordinator.start_isolation(
            isolatable_memtable,
            &age_info.collection_id,
            IsolationReason::Flush,
        ).await?;
        
        // Trigger actual flush operation (this would integrate with existing flush logic)
        // For now, we just log the trigger - the actual flush would be handled by the flush coordinator
        tracing::info!("🚀 Age-based flush isolation started for collection {}", age_info.collection_id);
        
        Ok(())
    }
    
    /// Update monitoring statistics
    async fn update_stats(&self, checked_count: usize, triggered_count: usize, duration_ms: f64, oldest_ages: HashMap<CollectionId, Duration>) {
        let mut stats = self.stats.write().await;
        
        stats.total_checks += 1;
        stats.age_triggered_flushes += triggered_count as u64;
        stats.monitored_collections = checked_count;
        stats.last_check_at = Some(Utc::now());
        
        // Update average check duration (exponential moving average)
        let alpha = 0.1; // smoothing factor
        if stats.avg_check_duration_ms == 0.0 {
            stats.avg_check_duration_ms = duration_ms;
        } else {
            stats.avg_check_duration_ms = alpha * duration_ms + (1.0 - alpha) * stats.avg_check_duration_ms;
        }
        
        stats.oldest_collection_ages = oldest_ages;
    }
    
    /// Get current monitoring statistics
    pub async fn get_stats(&self) -> AgeMonitorStats {
        self.stats.read().await.clone()
    }
    
    /// Manually trigger age check (for testing or immediate checking)
    pub async fn manual_age_check(&self) -> Result<Vec<CollectionAgeInfo>> {
        let memtable = self.wal_manager.get_memtable_strategy().await?;
        let collection_ages = memtable.get_collection_ages().await?;
        
        let mut age_infos = Vec::new();
        
        for (collection_id, age) in collection_ages {
            let threshold_secs = self.config.effective_max_age_for_collection(&collection_id);
            let threshold = Duration::seconds(threshold_secs as i64);
            let needs_flush = age > threshold;
            
            let oldest_timestamp = memtable.get_oldest_unflushed_timestamp(&collection_id).await?;
            let stats = memtable.get_stats().await?;
            let collection_stats = stats.get(&collection_id);
            
            let age_info = CollectionAgeInfo {
                collection_id: collection_id.clone(),
                oldest_entry_timestamp: oldest_timestamp.unwrap_or_else(Utc::now),
                age,
                threshold,
                needs_flush,
                entry_count: collection_stats.map(|s| s.total_entries).unwrap_or(0),
                memory_usage: collection_stats.map(|s| s.memory_bytes).unwrap_or(0),
            };
            
            age_infos.push(age_info);
        }
        
        Ok(age_infos)
    }
}

/// WAL manager trait extension for age monitoring
#[async_trait::async_trait]
pub trait WalManagerTrait: Send + Sync {
    /// Get the memtable strategy for age monitoring
    async fn get_memtable_strategy(&self) -> Result<Arc<dyn MemTableStrategy>>;
    
    /// Get memtable as Any for downcasting
    fn as_any(&self) -> &dyn std::any::Any;
}