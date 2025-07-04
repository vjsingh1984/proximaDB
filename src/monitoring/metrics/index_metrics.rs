// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Index-level metrics collection

use anyhow::Result;
use std::sync::Arc;

use super::registry::{Counter, Gauge, HistogramMetric, MetricsRegistry};

/// Index metrics collector
pub struct IndexMetricsCollector {
    #[allow(dead_code)]
    registry: Arc<MetricsRegistry>,

    // Metrics
    total_indexes_gauge: Arc<Gauge>,
    index_memory_usage_gauge: Arc<Gauge>,
    index_build_operations_counter: Arc<Counter>,
    index_rebuild_operations_counter: Arc<Counter>,
    search_operations_counter: Arc<Counter>,
    search_latency_histogram: Arc<HistogramMetric>,
    vector_insertions_counter: Arc<Counter>,
    vector_deletions_counter: Arc<Counter>,
}

impl IndexMetricsCollector {
    /// Create new index metrics collector
    pub async fn new(registry: Arc<MetricsRegistry>) -> Result<Self> {
        // Register all index metrics
        let total_indexes_gauge = registry
            .register_gauge(
                "proximadb_total_indexes",
                "Total number of indexes",
                std::collections::HashMap::new(),
            )
            .await?;

        let index_memory_usage_gauge = registry
            .register_gauge(
                "proximadb_index_memory_usage_bytes",
                "Index memory usage in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let index_build_operations_counter = registry
            .register_counter(
                "proximadb_index_build_operations_total",
                "Total number of index build operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let index_rebuild_operations_counter = registry
            .register_counter(
                "proximadb_index_rebuild_operations_total",
                "Total number of index rebuild operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let search_operations_counter = registry
            .register_counter(
                "proximadb_search_operations_total",
                "Total number of search operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let search_latency_histogram = registry
            .register_histogram(
                "proximadb_search_latency_seconds",
                "Search operation latency in seconds",
                vec![
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
                ],
                std::collections::HashMap::new(),
            )
            .await?;

        let vector_insertions_counter = registry
            .register_counter(
                "proximadb_vector_insertions_total",
                "Total number of vector insertions",
                std::collections::HashMap::new(),
            )
            .await?;

        let vector_deletions_counter = registry
            .register_counter(
                "proximadb_vector_deletions_total",
                "Total number of vector deletions",
                std::collections::HashMap::new(),
            )
            .await?;

        Ok(Self {
            registry,
            total_indexes_gauge,
            index_memory_usage_gauge,
            index_build_operations_counter,
            index_rebuild_operations_counter,
            search_operations_counter,
            search_latency_histogram,
            vector_insertions_counter,
            vector_deletions_counter,
        })
    }

    /// Update index count
    pub async fn set_index_count(&self, count: u32) {
        self.total_indexes_gauge.set(count as f64).await;
    }

    /// Update index memory usage
    pub async fn set_index_memory_usage(&self, bytes: u64) {
        self.index_memory_usage_gauge.set(bytes as f64).await;
    }

    /// Record index build operation
    pub async fn record_index_build(&self) {
        self.index_build_operations_counter.inc().await;
    }

    /// Record index rebuild operation
    pub async fn record_index_rebuild(&self) {
        self.index_rebuild_operations_counter.inc().await;
    }

    /// Record search operation with latency
    pub async fn record_search(&self, latency_seconds: f64) {
        self.search_operations_counter.inc().await;
        self.search_latency_histogram.observe(latency_seconds).await;
    }

    /// Record vector insertion
    pub async fn record_vector_insertion(&self) {
        self.vector_insertions_counter.inc().await;
    }

    /// Record vector deletion
    pub async fn record_vector_deletion(&self) {
        self.vector_deletions_counter.inc().await;
    }
}
