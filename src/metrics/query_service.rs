// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Read-only metrics query service for external users

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::MetricsConfig;
use super::schema::{CollectionMetrics, GlobalMetrics, OptimizationHint, QueryOptimizationHints};
use super::store::MetricsPersistenceLayer;

/// Read-only service for querying metrics
pub struct MetricsQueryService {
    /// Persistent store
    store: Arc<MetricsPersistenceLayer>,

    /// Configuration
    config: MetricsConfig,

    /// LRU cache for frequently accessed metrics
    cache: Arc<RwLock<MetricsCache>>,
}

/// Simple LRU cache for metrics
struct MetricsCache {
    /// Cached collection metrics
    collections: HashMap<String, CachedMetrics>,

    /// Cached global metrics
    global: Option<CachedGlobalMetrics>,

    /// Maximum cache size in bytes
    max_size_bytes: usize,

    /// Current cache size in bytes
    current_size_bytes: usize,
}

/// Cached metrics with TTL
struct CachedMetrics {
    metrics: CollectionMetrics,
    hints: Vec<OptimizationHint>,
    cached_at: i64,
    ttl_seconds: i64,
}

/// Cached global metrics
struct CachedGlobalMetrics {
    metrics: GlobalMetrics,
    cached_at: i64,
    ttl_seconds: i64,
}

/// Options for querying metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsQueryOptions {
    /// Include query optimization hints
    pub include_hints: bool,

    /// Include historical data
    pub include_history: bool,

    /// Time range for historical data (if included)
    pub from_timestamp: Option<i64>,
    pub to_timestamp: Option<i64>,

    /// Specific metrics to include (if empty, include all)
    pub metric_names: Vec<String>,
}

impl Default for MetricsQueryOptions {
    fn default() -> Self {
        Self {
            include_hints: true,
            include_history: false,
            from_timestamp: None,
            to_timestamp: None,
            metric_names: Vec::new(),
        }
    }
}

impl MetricsQueryService {
    /// Create a new metrics query service
    pub async fn new(store: Arc<MetricsPersistenceLayer>, config: MetricsConfig) -> Result<Self> {
        let max_cache_bytes = config.max_memory_mb * 1024 * 1024;

        let cache = Arc::new(RwLock::new(MetricsCache {
            collections: HashMap::new(),
            global: None,
            max_size_bytes: max_cache_bytes,
            current_size_bytes: 0,
        }));

        info!(
            "Initialized MetricsQueryService with {}MB cache_info",
            config.max_memory_mb
        );

        Ok(Self {
            store,
            config,
            cache,
        })
    }

    /// Get global metrics
    pub async fn global_metrics(&self) -> Result<GlobalMetrics> {
        // Check cache first
        let cache = self.cache.read().await;
        if let Some(cached) = &cache.global {
            if self.is_cache_valid(cached.cached_at, cached.ttl_seconds) {
                debug!("Returning cached global metrics");
                return Ok(cached.metrics.clone());
            }
        }
        drop(cache);

        // Load from store
        let metrics = self.store.global_metrics().await?;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.global = Some(CachedGlobalMetrics {
            metrics: metrics.clone(),
            cached_at: chrono::Utc::now().timestamp(),
            ttl_seconds: 60, // 1 minute TTL for global metrics
        });

        Ok(metrics)
    }

    /// Get metrics for a specific collection
    pub async fn collection_metrics(
        &self,
        collection_id: &str,
        options: MetricsQueryOptions,
    ) -> Result<serde_json::Value> {
        // Check cache first
        let cache = self.cache.read().await;
        if let Some(cached) = cache.collections.get(collection_id) {
            if self.is_cache_valid(cached.cached_at, cached.ttl_seconds) {
                debug!("Returning cached metrics for collection {}", collection_id);
                return self.format_metrics_response(
                    &cached.metrics,
                    if options.include_hints {
                        Some(&cached.hints)
                    } else {
                        None
                    },
                );
            }
        }
        drop(cache);

        // Load from store
        let metrics = self
            .store
            .collection_metrics(collection_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;

        // Generate optimization hints if requested
        let hints = if options.include_hints {
            metrics.generate_hints(&self.config)
        } else {
            Vec::new()
        };

        // Update cache
        self.update_cache(collection_id, metrics.clone(), hints.clone())
            .await?;

        // Format response
        self.format_metrics_response(
            &metrics,
            if options.include_hints {
                Some(&hints)
            } else {
                None
            },
        )
    }

    /// Get query optimization hints for a collection
    pub async fn query_hints(
        &self,
        collection_id: &str,
        query_type: Option<String>,
    ) -> Result<QueryOptimizationHints> {
        let metrics = self
            .store
            .collection_metrics(collection_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;

        let mut hints = metrics.generate_hints(&self.config);

        // Filter hints by query type if specified
        if let Some(qtype) = query_type {
            hints.retain(|hint| {
                hint.applicable_queries
                    .iter()
                    .any(|q| q == &qtype || q == "all")
            });
        }

        Ok(QueryOptimizationHints {
            collection_id: collection_id.to_string(),
            hints,
            generated_at: chrono::Utc::now().timestamp_millis(),
        })
    }

    /// Get metrics for all collections (summary only)
    pub async fn all_collections_summary(&self) -> Result<Vec<serde_json::Value>> {
        let snapshots = self.store.load_all_snapshots().await?;

        let mut summaries = Vec::new();
        for (collection_id, snapshot) in snapshots {
            let summary = serde_json::json!({
                "collection_id": collection_id,
                "vector_count": snapshot.metrics.vector_count,
                "dimension": snapshot.metrics.dimension,
                "data_size_bytes": snapshot.metrics.data_size_bytes,
                "total_operations": snapshot.metrics.total_inserts
                    + snapshot.metrics.total_updates
                    + snapshot.metrics.total_deletes
                    + snapshot.metrics.total_searches,
                "last_updated": snapshot.metrics.updated_at,
                "cache_hit_ratio": snapshot.metrics.cache_hit_ratio,
            });
            summaries.push(summary);
        }

        Ok(summaries)
    }

    /// Check if cached data is still valid
    fn is_cache_valid(&self, cached_at: i64, ttl_seconds: i64) -> bool {
        let now = chrono::Utc::now().timestamp();
        now - cached_at < ttl_seconds
    }

    /// Update the cache with new metrics
    async fn update_cache(
        &self,
        collection_id: &str,
        metrics: CollectionMetrics,
        hints: Vec<OptimizationHint>,
    ) -> Result<()> {
        let mut cache = self.cache.write().await;

        // Estimate size (rough approximation)
        let estimated_size = std::mem::size_of::<CollectionMetrics>()
            + hints.len() * std::mem::size_of::<OptimizationHint>();

        // Check if we need to evict entries (simple LRU)
        if cache.current_size_bytes + estimated_size > cache.max_size_bytes {
            // Find and remove oldest entry
            if let Some(oldest_key) = cache
                .collections
                .iter()
                .min_by_key(|(_, v)| v.cached_at)
                .map(|(k, _)| k.clone())
            {
                cache.collections.remove(&oldest_key);
                cache.current_size_bytes = cache.current_size_bytes.saturating_sub(estimated_size);
            }
        }

        // Add new entry
        cache.collections.insert(
            collection_id.to_string(),
            CachedMetrics {
                metrics,
                hints,
                cached_at: chrono::Utc::now().timestamp(),
                ttl_seconds: 300, // 5 minute TTL for collection metrics
            },
        );
        cache.current_size_bytes += estimated_size;

        Ok(())
    }

    /// Format metrics response as JSON
    fn format_metrics_response(
        &self,
        metrics: &CollectionMetrics,
        hints: Option<&[OptimizationHint]>,
    ) -> Result<serde_json::Value> {
        let mut response = serde_json::json!({
            "collection_id": metrics.collection_id,
            "metrics": {
                "basic": {
                    "vector_count": metrics.vector_count,
                    "dimension": metrics.dimension,
                    "data_size_bytes": metrics.data_size_bytes,
                    "index_size_bytes": metrics.index_size_bytes,
                },
                "operations": {
                    "total_inserts": metrics.total_inserts,
                    "total_updates": metrics.total_updates,
                    "total_deletes": metrics.total_deletes,
                    "total_searches": metrics.total_searches,
                    "total_flushes": metrics.total_flushes,
                    "total_compactions": metrics.total_compactions,
                },
                "performance": {
                    "avg_insert_latency_us": metrics.avg_insert_latency_us,
                    "avg_search_latency_us": metrics.avg_search_latency_us,
                    "p50_search_latency_us": metrics.p50_search_latency_us,
                    "p95_search_latency_us": metrics.p95_search_latency_us,
                    "p99_search_latency_us": metrics.p99_search_latency_us,
                    "cache_hit_ratio": metrics.cache_hit_ratio,
                },
                "storage": {
                    "parquet_file_count": metrics.parquet_file_count,
                    "sstable_file_count": metrics.sstable_file_count,
                    "wal_size_bytes": metrics.wal_size_bytes,
                    "memtable_size_bytes": metrics.memtable_size_bytes,
                },
                "characteristics": {
                    "sparsity_ratio": metrics.sparsity_ratio,
                    "avg_vector_magnitude": metrics.avg_vector_magnitude,
                    "distinct_metadata_keys": metrics.distinct_metadata_keys,
                },
            },
            "last_updated": metrics.updated_at,
        });

        // Add hints if requested
        if let Some(hints) = hints {
            response["optimization_hints"] = serde_json::json!(hints);
        }

        // Add filterable column stats if present
        if !metrics.filterable_column_stats.is_empty() {
            let mut column_stats = serde_json::Map::new();
            for (col_name, stats) in &metrics.filterable_column_stats {
                column_stats.insert(
                    col_name.clone(),
                    serde_json::json!({
                        "cardinality": stats.cardinality,
                        "selectivity": stats.selectivity,
                        "null_count": stats.null_count,
                        "data_type": stats.data_type,
                    }),
                );
            }
            response["filterable_columns"] = serde_json::Value::Object(column_stats);
        }

        Ok(response)
    }
}
