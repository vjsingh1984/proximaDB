// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Query-level metrics collection

use anyhow::Result;
use std::sync::Arc;

use super::registry::{Counter, Gauge, HistogramMetric, MetricsRegistry};

/// Query metrics collector
pub struct QueryMetricsCollector {
    registry: Arc<MetricsRegistry>,

    // Metrics
    total_queries_counter: Arc<Counter>,
    successful_queries_counter: Arc<Counter>,
    failed_queries_counter: Arc<Counter>,
    query_latency_histogram: Arc<HistogramMetric>,
    queries_per_second_gauge: Arc<Gauge>,
    slow_queries_counter: Arc<Counter>,
    timeout_queries_counter: Arc<Counter>,
}

impl QueryMetricsCollector {
    /// Create new query metrics collector
    pub async fn new(registry: Arc<MetricsRegistry>) -> Result<Self> {
        // Register all query metrics
        let total_queries_counter = registry
            .register_counter(
                "proximadb_total_queries",
                "Total number of queries processed",
                std::collections::HashMap::new(),
            )
            .await?;

        let successful_queries_counter = registry
            .register_counter(
                "proximadb_successful_queries_total",
                "Total number of successful queries",
                std::collections::HashMap::new(),
            )
            .await?;

        let failed_queries_counter = registry
            .register_counter(
                "proximadb_failed_queries_total",
                "Total number of failed queries",
                std::collections::HashMap::new(),
            )
            .await?;

        let query_latency_histogram = registry
            .register_histogram(
                "proximadb_query_latency_seconds",
                "Query latency in seconds",
                vec![
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
                std::collections::HashMap::new(),
            )
            .await?;

        let queries_per_second_gauge = registry
            .register_gauge(
                "proximadb_queries_per_second",
                "Current queries per second",
                std::collections::HashMap::new(),
            )
            .await?;

        let slow_queries_counter = registry
            .register_counter(
                "proximadb_slow_queries_total",
                "Total number of slow queries (above threshold)",
                std::collections::HashMap::new(),
            )
            .await?;

        let timeout_queries_counter = registry
            .register_counter(
                "proximadb_timeout_queries_total",
                "Total number of queries that timed out",
                std::collections::HashMap::new(),
            )
            .await?;

        Ok(Self {
            registry,
            total_queries_counter,
            successful_queries_counter,
            failed_queries_counter,
            query_latency_histogram,
            queries_per_second_gauge,
            slow_queries_counter,
            timeout_queries_counter,
        })
    }

    /// Record successful query with latency
    pub async fn record_successful_query(&self, latency_seconds: f64, slow_threshold_seconds: f64) {
        self.total_queries_counter.inc().await;
        self.successful_queries_counter.inc().await;
        self.query_latency_histogram.observe(latency_seconds).await;

        if latency_seconds > slow_threshold_seconds {
            self.slow_queries_counter.inc().await;
        }
    }

    /// Record failed query
    pub async fn record_failed_query(&self, latency_seconds: f64) {
        self.total_queries_counter.inc().await;
        self.failed_queries_counter.inc().await;
        self.query_latency_histogram.observe(latency_seconds).await;
    }

    /// Record query timeout
    pub async fn record_timeout_query(&self) {
        self.total_queries_counter.inc().await;
        self.failed_queries_counter.inc().await;
        self.timeout_queries_counter.inc().await;
    }

    /// Update queries per second
    pub async fn set_queries_per_second(&self, qps: f64) {
        self.queries_per_second_gauge.set(qps).await;
    }

    /// Get total query count
    pub async fn get_total_queries(&self) -> f64 {
        self.total_queries_counter.get().await
    }

    /// Get successful query count
    pub async fn get_successful_queries(&self) -> f64 {
        self.successful_queries_counter.get().await
    }

    /// Get failed query count
    pub async fn get_failed_queries(&self) -> f64 {
        self.failed_queries_counter.get().await
    }

    /// Get error rate
    pub async fn get_error_rate(&self) -> f64 {
        let total = self.get_total_queries().await;
        let failed = self.get_failed_queries().await;

        if total > 0.0 {
            failed / total
        } else {
            0.0
        }
    }
}
