// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Storage-level metrics collection

use anyhow::Result;
use std::sync::Arc;

use super::registry::{Counter, Gauge, MetricsRegistry};

/// Storage metrics collector
pub struct StorageMetricsCollector {
    #[allow(dead_code)]
    registry: Arc<MetricsRegistry>,

    // Metrics
    total_vectors_gauge: Arc<Gauge>,
    total_collections_gauge: Arc<Gauge>,
    storage_size_gauge: Arc<Gauge>,
    wal_size_gauge: Arc<Gauge>,
    memtable_size_gauge: Arc<Gauge>,
    compaction_operations_counter: Arc<Counter>,
    flush_operations_counter: Arc<Counter>,
    read_operations_counter: Arc<Counter>,
    write_operations_counter: Arc<Counter>,
    cache_hit_counter: Arc<Counter>,
    cache_miss_counter: Arc<Counter>,
}

impl StorageMetricsCollector {
    /// Create new storage metrics collector
    pub async fn new(registry: Arc<MetricsRegistry>) -> Result<Self> {
        // Register all storage metrics
        let total_vectors_gauge = registry
            .register_gauge(
                "proximadb_total_vectors",
                "Total number of vectors stored",
                std::collections::HashMap::new(),
            )
            .await?;

        let total_collections_gauge = registry
            .register_gauge(
                "proximadb_total_collections",
                "Total number of collections",
                std::collections::HashMap::new(),
            )
            .await?;

        let storage_size_gauge = registry
            .register_gauge(
                "proximadb_storage_size_bytes",
                "Total storage size in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let wal_size_gauge = registry
            .register_gauge(
                "proximadb_wal_size_bytes",
                "WAL size in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let memtable_size_gauge = registry
            .register_gauge(
                "proximadb_memtable_size_bytes",
                "Memtable size in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let compaction_operations_counter = registry
            .register_counter(
                "proximadb_compaction_operations_total",
                "Total number of compaction operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let flush_operations_counter = registry
            .register_counter(
                "proximadb_flush_operations_total",
                "Total number of flush operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let read_operations_counter = registry
            .register_counter(
                "proximadb_read_operations_total",
                "Total number of read operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let write_operations_counter = registry
            .register_counter(
                "proximadb_write_operations_total",
                "Total number of write operations",
                std::collections::HashMap::new(),
            )
            .await?;

        let cache_hit_counter = registry
            .register_counter(
                "proximadb_cache_hit_total",
                "Total number of cache hits",
                std::collections::HashMap::new(),
            )
            .await?;

        let cache_miss_counter = registry
            .register_counter(
                "proximadb_cache_miss_total",
                "Total number of cache misses",
                std::collections::HashMap::new(),
            )
            .await?;

        Ok(Self {
            registry,
            total_vectors_gauge,
            total_collections_gauge,
            storage_size_gauge,
            wal_size_gauge,
            memtable_size_gauge,
            compaction_operations_counter,
            flush_operations_counter,
            read_operations_counter,
            write_operations_counter,
            cache_hit_counter,
            cache_miss_counter,
        })
    }

    /// Update vector count
    pub async fn set_vector_count(&self, count: u64) {
        self.total_vectors_gauge.set(count as f64).await;
    }

    /// Update collection count
    pub async fn set_collection_count(&self, count: u32) {
        self.total_collections_gauge.set(count as f64).await;
    }

    /// Update storage size
    pub async fn set_storage_size(&self, bytes: u64) {
        self.storage_size_gauge.set(bytes as f64).await;
    }

    /// Update WAL size
    pub async fn set_wal_size(&self, bytes: u64) {
        self.wal_size_gauge.set(bytes as f64).await;
    }

    /// Update memtable size
    pub async fn set_memtable_size(&self, bytes: u64) {
        self.memtable_size_gauge.set(bytes as f64).await;
    }

    /// Record compaction operation
    pub async fn record_compaction(&self) {
        self.compaction_operations_counter.inc().await;
    }

    /// Record flush operation
    pub async fn record_flush(&self) {
        self.flush_operations_counter.inc().await;
    }

    /// Record read operation
    pub async fn record_read(&self) {
        self.read_operations_counter.inc().await;
    }

    /// Record write operation
    pub async fn record_write(&self) {
        self.write_operations_counter.inc().await;
    }

    /// Record cache hit
    pub async fn record_cache_hit(&self) {
        self.cache_hit_counter.inc().await;
    }

    /// Record cache miss
    pub async fn record_cache_miss(&self) {
        self.cache_miss_counter.inc().await;
    }

    /// Get cache hit rate
    pub async fn get_cache_hit_rate(&self) -> f64 {
        let hits = self.cache_hit_counter.get().await;
        let misses = self.cache_miss_counter.get().await;
        let total = hits + misses;

        if total > 0.0 {
            hits / total
        } else {
            0.0
        }
    }
}
