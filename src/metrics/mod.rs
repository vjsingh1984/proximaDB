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

pub use store::{PersistentMetricsStore, MetricsSnapshot};
pub use updater::{InternalMetricsUpdater, MetricsUpdate};
pub use query_service::{MetricsQueryService, MetricsQueryOptions};
pub use schema::{CollectionMetrics, GlobalMetrics, QueryOptimizationHints};
pub use aggregator::{MetricsAggregator, AggregationWindow};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for the metrics system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Base path for metrics storage (e.g., "s3://bucket/metrics" or "file:///data/metrics")
    pub storage_path: String,
    
    /// Snapshot frequency in seconds (min: 300/5min, default: 1800/30min)
    pub snapshot_interval_seconds: u64,
    
    /// Retention period in days (max: 30, default: 7)
    pub retention_days: u32,
    
    /// Maximum memory for metrics cache in MB (max: 1024, default: 100)
    pub max_memory_mb: usize,
    
    /// Enable query optimization hints
    pub enable_query_hints: bool,
    
    /// Enable automatic aggregations (hourly, daily)
    pub enable_aggregations: bool,
    
    /// Threshold for parallel scan optimization (number of files)
    pub parallel_scan_threshold: usize,
    
    /// Sparsity threshold for compression decisions (% of zero/null values)
    pub sparsity_threshold: f32,
    
    /// Size threshold for quantization recommendations (bytes)
    pub quantization_size_threshold: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            storage_path: "file:///data/proximadb/metrics".to_string(),
            snapshot_interval_seconds: 1800, // 30 minutes
            retention_days: 7,
            max_memory_mb: 100,
            enable_query_hints: true,
            enable_aggregations: true,
            parallel_scan_threshold: 10, // Suggest parallel scan if >10 files
            sparsity_threshold: 0.3,     // Consider sparse if >30% zeros
            quantization_size_threshold: 100 * 1024 * 1024, // 100MB
        }
    }
}

impl MetricsConfig {
    /// Validate and adjust configuration to safe bounds
    pub fn validate(&mut self) -> Result<()> {
        // Enforce minimum snapshot interval
        if self.snapshot_interval_seconds < 300 {
            tracing::warn!("Snapshot interval {} too low, setting to minimum 300 seconds", 
                self.snapshot_interval_seconds);
            self.snapshot_interval_seconds = 300;
        }
        
        // Enforce maximum retention
        if self.retention_days > 30 {
            tracing::warn!("Retention period {} days too high, setting to maximum 30 days", 
                self.retention_days);
            self.retention_days = 30;
        }
        
        // Enforce maximum memory
        if self.max_memory_mb > 1024 {
            tracing::warn!("Max memory {}MB too high, setting to maximum 1024MB", 
                self.max_memory_mb);
            self.max_memory_mb = 1024;
        }
        
        Ok(())
    }
}