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
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Internal interface for updating metrics - not exposed to external users
#[async_trait]
pub trait InternalMetricsUpdater: Send + Sync {
    /// Record a vector operation (insert/update/delete/search)
    async fn record_operation(
        &self,
        collection_id: &str,
        update: OperationMetricsUpdate,
    ) -> Result<()>;
    
    /// Record search metrics
    async fn record_search(
        &self,
        collection_id: &str,
        update: SearchMetricsUpdate,
    ) -> Result<()>;
    
    /// Update metrics after a flush operation
    async fn record_flush(
        &self,
        collection_id: &str,
        update: FlushMetricsUpdate,
    ) -> Result<()>;
    
    /// Update metrics after a compaction operation
    async fn record_compaction(
        &self,
        collection_id: &str,
        update: CompactionMetricsUpdate,
    ) -> Result<()>;
    
    /// Update storage metrics
    async fn update_storage_metrics(
        &self,
        collection_id: &str,
        update: StorageMetricsUpdate,
    ) -> Result<()>;
    
    /// Update data characteristics for optimization
    async fn update_data_characteristics(
        &self,
        collection_id: &str,
        update: DataCharacteristicsUpdate,
    ) -> Result<()>;
}

/// Types of vector operations
#[derive(Debug, Clone, Copy)]
pub enum OperationType {
    Insert,
    Update,
    Delete,
    Search,
}

/// Metrics update from vector operations
#[derive(Debug, Clone)]
pub struct OperationMetricsUpdate {
    pub operation_type: OperationType,
    pub vector_count: i32,
    pub latency_ms: i64,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Metrics update from search operations
#[derive(Debug, Clone)]
pub struct SearchMetricsUpdate {
    pub query_vector_dimension: i32,
    pub k: i32,
    pub results_returned: i32,
    pub latency_ms: i64,
    pub engine_used: String, // "VIPER" or "SST"
    pub used_index: bool,
    pub filter_applied: bool,
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
    pub vectors_merged: i64,
    pub vectors_deleted: i64,
    pub engine_type: String, // "VIPER" or "SST"
    pub timestamp: i64,
}

/// Storage metrics update
#[derive(Debug, Clone)]
pub struct StorageMetricsUpdate {
    pub total_vectors: i64,
    pub total_bytes: i64,
    pub file_count: i32,
    pub memtable_bytes: i64,
    pub disk_bytes: i64,
    pub cache_bytes: i64,
    pub cache_hit_rate: f32,
}

/// Data characteristics for query optimization
#[derive(Debug, Clone)]
pub struct DataCharacteristicsUpdate {
    pub vector_dimension: i32,
    pub total_vectors: i64,
    pub cardinality: i64,
    pub sparsity: f32,
    pub value_distribution: String, // "uniform", "gaussian", "skewed"
    pub clustering_coefficient: f32,
    pub metadata_selectivity: f32,
}

/// No-op implementation for when metrics are disabled
pub struct NoOpMetricsUpdater;

#[async_trait]
impl InternalMetricsUpdater for NoOpMetricsUpdater {
    async fn record_operation(&self, _: &str, _: OperationMetricsUpdate) -> Result<()> {
        Ok(())
    }
    
    async fn record_search(&self, _: &str, _: SearchMetricsUpdate) -> Result<()> {
        Ok(())
    }
    
    async fn record_flush(&self, _: &str, _: FlushMetricsUpdate) -> Result<()> {
        Ok(())
    }
    
    async fn record_compaction(&self, _: &str, _: CompactionMetricsUpdate) -> Result<()> {
        Ok(())
    }
    
    async fn update_storage_metrics(&self, _: &str, _: StorageMetricsUpdate) -> Result<()> {
        Ok(())
    }
    
    async fn update_data_characteristics(&self, _: &str, _: DataCharacteristicsUpdate) -> Result<()> {
        Ok(())
    }
}