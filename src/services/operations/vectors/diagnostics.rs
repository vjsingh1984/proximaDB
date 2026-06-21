//! Diagnostics & metrics collaborator extracted from `VectorOperationsService`
//! (Phase 2.1 god-object decomposition).
//!
//! Read-only observability over the WAL, the default storage engine, and the
//! collection cache — deliberately isolated from the hot search/write paths.
//! `VectorOperationsService` keeps its public (and `VectorOpsPort`) surface and
//! delegates the metrics / health / unflushed-record accessors here.

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use proximadb_records::ProximaRecord;

use crate::proto::proximadb_v1::Collection;
use crate::storage::engines::sst::SstEngine;
use crate::storage::persistence::write_ahead_log::WriteAheadLogManager;
// Brings `health_check` into scope on the SST engine handle.
use crate::storage::traits::UnifiedStorageFormat;

/// Observability surface for the vector store: WAL/engine/cache stats, a health
/// roll-up, and unflushed-record inspection. Holds only cheap `Arc` handles to
/// the subsystems it reports on, so it can be constructed on demand per call.
pub(crate) struct VectorServiceDiagnostics {
    wal_manager: Arc<WriteAheadLogManager>,
    storage_engine: Arc<SstEngine>,
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,
}

impl VectorServiceDiagnostics {
    pub(crate) fn new(
        wal_manager: Arc<WriteAheadLogManager>,
        storage_engine: Arc<SstEngine>,
        collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    ) -> Self {
        Self {
            wal_manager,
            storage_engine,
            collection_cache,
        }
    }

    /// Collect a JSON snapshot of key operational metrics (WAL, storage, query
    /// cache, and collection counts).
    pub(crate) async fn metrics(&self) -> Result<serde_json::Value> {
        // Collect metrics from various components
        let wal_stats = self.wal_manager.stats().await?;

        // Get storage engine metrics
        let storage_metrics = match self.storage_engine.health_check().await {
            Ok(health) => serde_json::json!({
                "status": health.status,
                "response_time_ms": health.response_time_ms,
                "healthy": health.healthy,
                "warnings": health.warnings
            }),
            Err(e) => serde_json::json!({
                "status": "error",
                "error": e.to_string()
            }),
        };

        // Get query cache metrics - not implemented yet
        let cache_stats = serde_json::json!({
            "hit_rate": 0.0,
            "total_queries": 0,
            "cache_hits": 0,
            "cache_misses": 0
        });

        // Combine all metrics
        Ok(serde_json::json!({
            "wal": {
                "total_entries": wal_stats.total_entries,
                "memory_entries": wal_stats.memory_entries,
                "disk_segments": wal_stats.disk_segments,
                "total_disk_size_bytes": wal_stats.total_disk_size_bytes,
                "memory_size_bytes": wal_stats.memory_size_bytes,
            },
            "storage": storage_metrics,
            "query_cache": cache_stats,
            "collections": self.collection_cache.len(),
        }))
    }

    /// Perform a health check across all subsystems (WAL, storage engine, query
    /// cache) and return a JSON report.
    pub(crate) async fn health_check(&self) -> Result<serde_json::Value> {
        let _status = "healthy";
        let issues: Vec<String> = Vec::new();

        // Check WAL health
        let wal_health = match self.wal_manager.stats().await {
            Ok(stats) => {
                let memory_usage_mb = stats.memory_size_bytes as f64 / (1024.0 * 1024.0);
                if memory_usage_mb > 500.0 {
                    // More than 500MB in memory
                    vec![format!("High WAL memory usage: {:.1}MB", memory_usage_mb)]
                } else {
                    vec![]
                }
            }
            Err(e) => vec![format!("WAL stats error: {}", e)],
        };

        // Check storage engine health
        let storage_health = match self.storage_engine.health_check().await {
            Ok(engine_health) => match engine_health.status.as_str() {
                "healthy" => vec![],
                _ => vec![format!("Storage engine: {}", engine_health.status)],
            },
            Err(e) => vec![format!("Storage engine health check failed: {}", e)],
        };

        // Combine health issues
        let mut all_issues = issues;
        all_issues.extend(wal_health);
        all_issues.extend(storage_health);

        // Update status based on issues
        let status = if all_issues.is_empty() {
            "healthy"
        } else {
            "degraded"
        };

        Ok(serde_json::json!({
            "status": status,
            "issues": all_issues,
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "collections": self.collection_cache.len(),
        }))
    }

    /// Get unflushed vectors for a collection from the WAL/memtable.
    pub(crate) async fn get_unflushed_vectors(
        &self,
        collection_id: &str,
    ) -> Result<Vec<ProximaRecord>> {
        self.wal_manager
            .read_record_entries(collection_id, 0, None)
            .await
    }
}
