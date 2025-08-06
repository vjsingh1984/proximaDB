// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Internal-only metrics update interface
//! 
//! This module provides the write path for metrics that is only accessible
//! to internal system components. All updates are non-blocking and failure-tolerant.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, warn};

use super::schema::{CollectionMetrics, FilterableColumnStats};

/// Internal interface for updating metrics - not exposed to external users
#[async_trait]
pub trait InternalMetricsUpdater: Send + Sync {
    /// Record a vector operation (insert/update/delete/search)
    async fn record_operation(
        &self,
        collection_id: &str,
        operation: OperationType,
        latency_us: u64,
        success: bool,
    );
    
    /// Update metrics after a flush operation
    async fn record_flush(
        &self,
        collection_id: &str,
        update: FlushMetricsUpdate,
    );
    
    /// Update metrics after a compaction operation
    async fn record_compaction(
        &self,
        collection_id: &str,
        update: CompactionMetricsUpdate,
    );
    
    /// Update storage metrics
    async fn update_storage_metrics(
        &self,
        collection_id: &str,
        update: StorageMetricsUpdate,
    );
    
    /// Update data characteristics for optimization
    async fn update_data_characteristics(
        &self,
        collection_id: &str,
        update: DataCharacteristicsUpdate,
    );
    
    /// Update filterable column statistics
    async fn update_column_stats(
        &self,
        collection_id: &str,
        column_name: &str,
        stats: FilterableColumnStats,
    );
}

/// Types of vector operations
#[derive(Debug, Clone, Copy)]
pub enum OperationType {
    Insert,
    Update,
    Delete,
    Search,
}

/// Metrics update from flush operations
#[derive(Debug, Clone)]
pub struct FlushMetricsUpdate {
    pub vectors_flushed: i64,
    pub bytes_written: i64,
    pub duration_ms: i64,
    pub files_created: i32,
    pub engine_type: String, // "VIPER" or "SST"
    pub timestamp: i64,
}

/// Metrics update from compaction operations
#[derive(Debug, Clone)]
pub struct CompactionMetricsUpdate {
    pub files_before: i32,
    pub files_after: i32,
    pub bytes_before: i64,
    pub bytes_after: i64,
    pub duration_ms: i64,
    pub timestamp: i64,
}

/// Storage layer metrics update
#[derive(Debug, Clone)]
pub struct StorageMetricsUpdate {
    pub parquet_file_count: Option<i32>,
    pub sstable_file_count: Option<i32>,
    pub wal_size_bytes: Option<i64>,
    pub memtable_size_bytes: Option<i64>,
    pub bloom_filter_size_bytes: Option<i64>,
    pub cache_size_bytes: Option<i64>,
}

/// Data characteristics update for optimization
#[derive(Debug, Clone)]
pub struct DataCharacteristicsUpdate {
    pub sparsity_ratio: Option<f32>,
    pub avg_vector_magnitude: Option<f32>,
    pub distinct_metadata_keys: Option<i32>,
    pub avg_metadata_size_bytes: Option<i32>,
}

/// Generic metrics update message
#[derive(Debug, Clone)]
pub enum MetricsUpdate {
    Operation {
        collection_id: String,
        operation: OperationType,
        latency_us: u64,
        success: bool,
    },
    Flush {
        collection_id: String,
        update: FlushMetricsUpdate,
    },
    Compaction {
        collection_id: String,
        update: CompactionMetricsUpdate,
    },
    Storage {
        collection_id: String,
        update: StorageMetricsUpdate,
    },
    DataCharacteristics {
        collection_id: String,
        update: DataCharacteristicsUpdate,
    },
    ColumnStats {
        collection_id: String,
        column_name: String,
        stats: FilterableColumnStats,
    },
}

/// Async metrics updater that processes updates in the background
pub struct AsyncMetricsUpdater {
    /// Channel for sending updates
    tx: mpsc::UnboundedSender<MetricsUpdate>,
    
    /// Metrics store (shared with reader)
    metrics_cache: Arc<RwLock<std::collections::HashMap<String, CollectionMetrics>>>,
    
    /// Error counter for monitoring
    error_count: Arc<AtomicU64>,
    
    /// Update counter for monitoring
    update_count: Arc<AtomicU64>,
}

impl AsyncMetricsUpdater {
    /// Create a new async metrics updater
    pub fn new(
        metrics_cache: Arc<RwLock<std::collections::HashMap<String, CollectionMetrics>>>,
    ) -> (Self, mpsc::UnboundedReceiver<MetricsUpdate>) {
        let (tx, rx) = mpsc::unbounded_channel();
        
        let updater = Self {
            tx,
            metrics_cache,
            error_count: Arc::new(AtomicU64::new(0)),
            update_count: Arc::new(AtomicU64::new(0)),
        };
        
        (updater, rx)
    }
    
    /// Process updates from the channel
    pub async fn process_updates(
        &self,
        mut rx: mpsc::UnboundedReceiver<MetricsUpdate>,
        store: Arc<super::store::PersistentMetricsStore>,
    ) {
        while let Some(update) = rx.recv().await {
            self.update_count.fetch_add(1, Ordering::Relaxed);
            
            // Apply update to in-memory cache
            if let Err(e) = self.apply_update(&update).await {
                warn!("Failed to apply metrics update (non-critical): {}", e);
                self.error_count.fetch_add(1, Ordering::Relaxed);
            }
            
            // Optionally persist to store (async, best-effort)
            if let Err(e) = store.record_update(update).await {
                debug!("Failed to persist metrics update (non-critical): {}", e);
                // Don't increment error counter for persistence failures
            }
        }
    }
    
    /// Apply an update to the in-memory metrics cache
    async fn apply_update(&self, update: &MetricsUpdate) -> Result<()> {
        let mut cache = self.metrics_cache.write().await;
        
        match update {
            MetricsUpdate::Operation { collection_id, operation, latency_us, success } => {
                let metrics = cache.entry(collection_id.clone())
                    .or_insert_with(|| CollectionMetrics {
                        collection_id: collection_id.clone(),
                        ..Default::default()
                    });
                
                match operation {
                    OperationType::Insert => {
                        metrics.total_inserts += 1;
                        if *success {
                            // Update insert latency (simple moving average)
                            let weight = 0.1; // Weight for new value
                            metrics.avg_insert_latency_us = 
                                metrics.avg_insert_latency_us * (1.0 - weight) + 
                                (*latency_us as f64) * weight;
                        }
                    }
                    OperationType::Update => metrics.total_updates += 1,
                    OperationType::Delete => metrics.total_deletes += 1,
                    OperationType::Search => {
                        metrics.total_searches += 1;
                        if *success {
                            // Update search latency (simple moving average)
                            let weight = 0.1;
                            metrics.avg_search_latency_us = 
                                metrics.avg_search_latency_us * (1.0 - weight) + 
                                (*latency_us as f64) * weight;
                        }
                    }
                }
                
                metrics.updated_at = chrono::Utc::now().timestamp_millis();
            }
            
            MetricsUpdate::Flush { collection_id, update } => {
                let metrics = cache.entry(collection_id.clone())
                    .or_insert_with(|| CollectionMetrics {
                        collection_id: collection_id.clone(),
                        ..Default::default()
                    });
                
                metrics.total_flushes += 1;
                metrics.vector_count += update.vectors_flushed;
                metrics.data_size_bytes += update.bytes_written;
                metrics.last_flush_timestamp = update.timestamp;
                metrics.last_flush_duration_ms = update.duration_ms;
                
                if update.engine_type == "VIPER" {
                    metrics.parquet_file_count += update.files_created;
                } else if update.engine_type == "SST" {
                    metrics.sstable_file_count += update.files_created;
                }
                
                metrics.updated_at = chrono::Utc::now().timestamp_millis();
            }
            
            MetricsUpdate::Compaction { collection_id, update } => {
                let metrics = cache.entry(collection_id.clone())
                    .or_insert_with(|| CollectionMetrics {
                        collection_id: collection_id.clone(),
                        ..Default::default()
                    });
                
                metrics.total_compactions += 1;
                metrics.last_compaction_timestamp = update.timestamp;
                metrics.last_compaction_duration_ms = update.duration_ms;
                
                // Update file counts
                let file_reduction = update.files_before - update.files_after;
                if metrics.parquet_file_count > 0 {
                    metrics.parquet_file_count -= file_reduction;
                }
                
                // Update size
                let size_reduction = update.bytes_before - update.bytes_after;
                if size_reduction > 0 {
                    metrics.data_size_bytes = metrics.data_size_bytes.saturating_sub(size_reduction);
                }
                
                metrics.updated_at = chrono::Utc::now().timestamp_millis();
            }
            
            MetricsUpdate::Storage { collection_id, update } => {
                let metrics = cache.entry(collection_id.clone())
                    .or_insert_with(|| CollectionMetrics {
                        collection_id: collection_id.clone(),
                        ..Default::default()
                    });
                
                if let Some(count) = update.parquet_file_count {
                    metrics.parquet_file_count = count;
                }
                if let Some(count) = update.sstable_file_count {
                    metrics.sstable_file_count = count;
                }
                if let Some(size) = update.wal_size_bytes {
                    metrics.wal_size_bytes = size;
                }
                if let Some(size) = update.memtable_size_bytes {
                    metrics.memtable_size_bytes = size;
                }
                if let Some(size) = update.bloom_filter_size_bytes {
                    metrics.bloom_filter_size_bytes = size;
                }
                if let Some(size) = update.cache_size_bytes {
                    metrics.cache_size_bytes = size;
                }
                
                metrics.updated_at = chrono::Utc::now().timestamp_millis();
            }
            
            MetricsUpdate::DataCharacteristics { collection_id, update } => {
                let metrics = cache.entry(collection_id.clone())
                    .or_insert_with(|| CollectionMetrics {
                        collection_id: collection_id.clone(),
                        ..Default::default()
                    });
                
                if let Some(ratio) = update.sparsity_ratio {
                    metrics.sparsity_ratio = ratio;
                }
                if let Some(mag) = update.avg_vector_magnitude {
                    metrics.avg_vector_magnitude = mag;
                }
                if let Some(keys) = update.distinct_metadata_keys {
                    metrics.distinct_metadata_keys = keys;
                }
                if let Some(size) = update.avg_metadata_size_bytes {
                    metrics.avg_metadata_size_bytes = size;
                }
                
                metrics.updated_at = chrono::Utc::now().timestamp_millis();
            }
            
            MetricsUpdate::ColumnStats { collection_id, column_name, stats } => {
                let metrics = cache.entry(collection_id.clone())
                    .or_insert_with(|| CollectionMetrics {
                        collection_id: collection_id.clone(),
                        ..Default::default()
                    });
                
                metrics.filterable_column_stats.insert(column_name.clone(), stats.clone());
                metrics.updated_at = chrono::Utc::now().timestamp_millis();
            }
        }
        
        Ok(())
    }
    
    /// Get current error count
    pub fn error_count(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }
    
    /// Get total update count
    pub fn update_count(&self) -> u64 {
        self.update_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl InternalMetricsUpdater for AsyncMetricsUpdater {
    async fn record_operation(
        &self,
        collection_id: &str,
        operation: OperationType,
        latency_us: u64,
        success: bool,
    ) {
        // Fire and forget - never block
        let _ = self.tx.send(MetricsUpdate::Operation {
            collection_id: collection_id.to_string(),
            operation,
            latency_us,
            success,
        });
    }
    
    async fn record_flush(
        &self,
        collection_id: &str,
        update: FlushMetricsUpdate,
    ) {
        let _ = self.tx.send(MetricsUpdate::Flush {
            collection_id: collection_id.to_string(),
            update,
        });
    }
    
    async fn record_compaction(
        &self,
        collection_id: &str,
        update: CompactionMetricsUpdate,
    ) {
        let _ = self.tx.send(MetricsUpdate::Compaction {
            collection_id: collection_id.to_string(),
            update,
        });
    }
    
    async fn update_storage_metrics(
        &self,
        collection_id: &str,
        update: StorageMetricsUpdate,
    ) {
        let _ = self.tx.send(MetricsUpdate::Storage {
            collection_id: collection_id.to_string(),
            update,
        });
    }
    
    async fn update_data_characteristics(
        &self,
        collection_id: &str,
        update: DataCharacteristicsUpdate,
    ) {
        let _ = self.tx.send(MetricsUpdate::DataCharacteristics {
            collection_id: collection_id.to_string(),
            update,
        });
    }
    
    async fn update_column_stats(
        &self,
        collection_id: &str,
        column_name: &str,
        stats: FilterableColumnStats,
    ) {
        let _ = self.tx.send(MetricsUpdate::ColumnStats {
            collection_id: collection_id.to_string(),
            column_name: column_name.to_string(),
            stats,
        });
    }
}