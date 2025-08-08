// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive Metrics Framework for ProximaDB
//! 
//! This module provides a production-ready metrics system with:
//! - Read-only REST API for external users
//! - Internal-only update interface for system components
//! - Persistent storage with filesystem abstraction (S3/GCS/ADLS/File)
//! - Collection-partitioned storage for O(1) lookups
//! - Query optimization hints based on data characteristics
//! - Non-critical path - failures don't affect operations

pub mod store;
pub mod updater;
pub mod query_service;
pub mod schema;
pub mod aggregator;
pub mod compression;
pub mod cache;

#[cfg(test)]
mod tests;

pub use store::{PersistentMetricsStore, MetricsSnapshot};
pub use updater::{InternalMetricsUpdater, MetricsUpdate};
pub use query_service::{MetricsQueryService, MetricsQueryOptions};
pub use schema::{CollectionMetrics, GlobalMetrics, QueryOptimizationHints};
pub use aggregator::{MetricsAggregator, AggregationWindow};
pub use compression::{CompressionMetrics, CompressionMetricsTracker, CompressionResult, DecompressionResult};
pub use cache::{IntegratedCacheMetrics, CacheMetricsAggregator, CacheOptimizationHints};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for the metrics system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Enable or disable the entire metrics system
    pub enabled: bool,
    
    /// Number of partitions for collection-based metrics storage
    pub collection_partitions: usize,
    
    /// Base path for metrics storage (e.g., "s3://bucket/metrics" or "file:///data/metrics")
    pub storage_path: String,
    
    /// Flush interval for metrics updates in seconds
    pub flush_interval_seconds: u64,
    
    /// Retention period in days (max: 30, default: 7)
    pub retention_days: u32,
    
    /// Threshold for parallel scan optimization (number of files)
    pub parallel_scan_threshold: usize,
    
    /// Sparsity threshold for compression decisions (% of zero/null values)
    pub sparsity_threshold: f32,
    
    /// Size threshold for quantization recommendations (bytes)
    pub quantization_size_threshold: u64,
    
    /// Snapshot interval for metrics aggregation in seconds
    pub snapshot_interval_seconds: u64,
    
    /// Maximum memory usage in MB for metrics cache
    pub max_memory_mb: usize,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            collection_partitions: 16,
            storage_path: "file:///data/proximadb/metrics".to_string(),
            flush_interval_seconds: 30, // 30 seconds
            retention_days: 7,
            parallel_scan_threshold: 10, // Suggest parallel scan if >10 files
            sparsity_threshold: 0.3,     // Consider sparse if >30% zeros
            quantization_size_threshold: 100 * 1024 * 1024, // 100MB
            snapshot_interval_seconds: 60, // 1 minute snapshots
            max_memory_mb: 512, // 512MB max memory for metrics cache
        }
    }
}

impl MetricsConfig {
    /// Validate and adjust configuration to safe bounds
    pub fn validate(&mut self) -> Result<()> {
        // Enforce minimum flush interval
        if self.flush_interval_seconds < 10 {
            tracing::warn!("Flush interval {} too low, setting to minimum 10 seconds", 
                self.flush_interval_seconds);
            self.flush_interval_seconds = 10;
        }
        
        // Enforce maximum retention
        if self.retention_days > 30 {
            tracing::warn!("Retention period {} days too high, setting to maximum 30 days", 
                self.retention_days);
            self.retention_days = 30;
        }
        
        // Enforce minimum partitions
        if self.collection_partitions < 1 {
            tracing::warn!("Collection partitions {} too low, setting to minimum 1", 
                self.collection_partitions);
            self.collection_partitions = 1;
        }
        
        // Enforce maximum partitions
        if self.collection_partitions > 256 {
            tracing::warn!("Collection partitions {} too high, setting to maximum 256", 
                self.collection_partitions);
            self.collection_partitions = 256;
        }
        
        Ok(())
    }
}