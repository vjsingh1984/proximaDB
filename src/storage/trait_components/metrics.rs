//! Storage Engine Metrics Trait
//!
//! Defines metrics collection and health check operations for storage engines.
//! This trait follows the Interface Segregation Principle by separating
//! observability concerns from core storage operations.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::storage::traits::{EngineHealth, EngineStatistics};

use super::StorageIdentity;

/// Metrics and health operations for storage engines
///
/// This trait provides observability into engine operations:
/// - Real-time metrics collection
/// - Health checks for monitoring
/// - Performance statistics
///
/// # Design Philosophy
///
/// - **Non-blocking**: Metrics collection should not impact performance
/// - **Extensible**: Engine-specific metrics via HashMap
/// - **Standardized**: Common metrics format across all engines
#[async_trait]
pub trait StorageMetrics: StorageIdentity + Send + Sync {
    /// Collect engine-specific metrics
    ///
    /// Returns a map of metric names to values. Common metrics include:
    /// - `vector_count`: Total vectors stored
    /// - `file_count`: Number of storage files
    /// - `memory_usage_bytes`: Current memory usage
    /// - `disk_usage_bytes`: Current disk usage
    ///
    /// Engines can add custom metrics specific to their implementation.
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>>;

    /// Get comprehensive engine statistics
    ///
    /// Aggregates metrics into a structured format with common fields.
    async fn get_engine_stats(&self) -> Result<EngineStatistics> {
        let engine_metrics = self.collect_engine_metrics().await?;

        Ok(EngineStatistics {
            engine_name: self.engine_name().to_string(),
            engine_version: self.engine_version().to_string(),
            total_storage_bytes: engine_metrics
                .get("disk_usage_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            memory_usage_bytes: engine_metrics
                .get("memory_usage_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            collection_count: engine_metrics
                .get("collection_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            last_flush: engine_metrics
                .get("last_flush_timestamp")
                .and_then(|v| v.as_i64())
                .and_then(DateTime::from_timestamp_millis),
            last_compaction: engine_metrics
                .get("last_compaction_timestamp")
                .and_then(|v| v.as_i64())
                .and_then(DateTime::from_timestamp_millis),
            pending_flushes: engine_metrics
                .get("pending_flushes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            pending_compactions: engine_metrics
                .get("pending_compactions")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            engine_specific: engine_metrics,
        })
    }

    /// Perform health check
    ///
    /// Returns detailed health status including:
    /// - Overall healthy/unhealthy status
    /// - Response time for the check
    /// - Error count in recent period
    /// - Warnings for degraded conditions
    async fn health_check(&self) -> Result<EngineHealth> {
        let start_time = std::time::Instant::now();

        let stats = self.get_engine_stats().await?;
        let response_time = start_time.elapsed().as_secs_f64() * 1000.0;

        let healthy = stats
            .engine_specific
            .get("is_healthy")
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // Default to healthy

        let error_count = stats
            .engine_specific
            .get("error_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let warnings = stats
            .engine_specific
            .get("warnings")
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, |arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            });

        Ok(EngineHealth {
            healthy,
            status: if healthy {
                format!("{} engine healthy", self.engine_name())
            } else {
                format!("{} engine unhealthy", self.engine_name())
            },
            last_check: Utc::now(),
            response_time_ms: response_time,
            error_count,
            warnings,
            metrics: stats.engine_specific,
        })
    }
}
