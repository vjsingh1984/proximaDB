// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Server-level metrics collection

use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::registry::{Counter, Gauge, MetricsRegistry};

/// Server metrics collector
pub struct ServerMetricsCollector {
    registry: Arc<MetricsRegistry>,
    start_time: Instant,

    // Metrics
    uptime_counter: Arc<Counter>,
    cpu_usage_gauge: Arc<Gauge>,
    memory_usage_gauge: Arc<Gauge>,
    memory_available_gauge: Arc<Gauge>,
    disk_usage_gauge: Arc<Gauge>,
    disk_available_gauge: Arc<Gauge>,
    network_bytes_in_counter: Arc<Counter>,
    network_bytes_out_counter: Arc<Counter>,
    open_connections_gauge: Arc<Gauge>,
    active_requests_gauge: Arc<Gauge>,

    // Connection tracking
    connection_count: Arc<RwLock<u32>>,
    request_count: Arc<RwLock<u32>>,
}

impl ServerMetricsCollector {
    /// Create new server metrics collector
    pub async fn new(registry: Arc<MetricsRegistry>) -> Result<Self> {
        let start_time = Instant::now();

        // Register all server metrics
        let uptime_counter = registry
            .register_counter(
                "proximadb_uptime_seconds",
                "Server uptime in seconds",
                std::collections::HashMap::new(),
            )
            .await?;

        let cpu_usage_gauge = registry
            .register_gauge(
                "proximadb_cpu_usage_percent",
                "CPU usage percentage",
                std::collections::HashMap::new(),
            )
            .await?;

        let memory_usage_gauge = registry
            .register_gauge(
                "proximadb_memory_usage_bytes",
                "Memory usage in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let memory_available_gauge = registry
            .register_gauge(
                "proximadb_memory_available_bytes",
                "Available memory in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let disk_usage_gauge = registry
            .register_gauge(
                "proximadb_disk_usage_bytes",
                "Disk usage in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let disk_available_gauge = registry
            .register_gauge(
                "proximadb_disk_available_bytes",
                "Available disk space in bytes",
                std::collections::HashMap::new(),
            )
            .await?;

        let network_bytes_in_counter = registry
            .register_counter(
                "proximadb_network_bytes_in_total",
                "Total bytes received from network",
                std::collections::HashMap::new(),
            )
            .await?;

        let network_bytes_out_counter = registry
            .register_counter(
                "proximadb_network_bytes_out_total",
                "Total bytes sent to network",
                std::collections::HashMap::new(),
            )
            .await?;

        let open_connections_gauge = registry
            .register_gauge(
                "proximadb_open_connections",
                "Number of open connections",
                std::collections::HashMap::new(),
            )
            .await?;

        let active_requests_gauge = registry
            .register_gauge(
                "proximadb_active_requests",
                "Number of active requests",
                std::collections::HashMap::new(),
            )
            .await?;

        Ok(Self {
            registry,
            start_time,
            uptime_counter,
            cpu_usage_gauge,
            memory_usage_gauge,
            memory_available_gauge,
            disk_usage_gauge,
            disk_available_gauge,
            network_bytes_in_counter,
            network_bytes_out_counter,
            open_connections_gauge,
            active_requests_gauge,
            connection_count: Arc::new(RwLock::new(0)),
            request_count: Arc::new(RwLock::new(0)),
        })
    }

    /// Update system resource metrics
    pub async fn update_system_metrics(
        &self,
        cpu_usage: f64,
        memory_usage: u64,
        memory_available: u64,
        disk_usage: u64,
        disk_available: u64,
        network_bytes_in: u64,
        network_bytes_out: u64,
    ) -> Result<()> {
        // Update uptime (reset and set new value for counter)
        let uptime = self.start_time.elapsed().as_secs_f64();
        self.uptime_counter.reset().await;
        self.uptime_counter.add(uptime).await;

        // Update resource metrics
        self.cpu_usage_gauge.set(cpu_usage).await;
        self.memory_usage_gauge.set(memory_usage as f64).await;
        self.memory_available_gauge
            .set(memory_available as f64)
            .await;
        self.disk_usage_gauge.set(disk_usage as f64).await;
        self.disk_available_gauge.set(disk_available as f64).await;

        // Network metrics (counters should only increase)
        let current_in = self.network_bytes_in_counter.get().await;
        let current_out = self.network_bytes_out_counter.get().await;

        if network_bytes_in > current_in as u64 {
            self.network_bytes_in_counter
                .add((network_bytes_in - current_in as u64) as f64)
                .await;
        }
        if network_bytes_out > current_out as u64 {
            self.network_bytes_out_counter
                .add((network_bytes_out - current_out as u64) as f64)
                .await;
        }

        // Update connection and request counts
        let connections = *self.connection_count.read().await;
        let requests = *self.request_count.read().await;

        self.open_connections_gauge.set(connections as f64).await;
        self.active_requests_gauge.set(requests as f64).await;

        Ok(())
    }

    /// Track new connection
    pub async fn connection_opened(&self) {
        let mut count = self.connection_count.write().await;
        *count += 1;
        self.open_connections_gauge.set(*count as f64).await;
    }

    /// Track connection closure
    pub async fn connection_closed(&self) {
        let mut count = self.connection_count.write().await;
        if *count > 0 {
            *count -= 1;
        }
        self.open_connections_gauge.set(*count as f64).await;
    }

    /// Track request start
    pub async fn request_started(&self) {
        let mut count = self.request_count.write().await;
        *count += 1;
        self.active_requests_gauge.set(*count as f64).await;
    }

    /// Track request completion
    pub async fn request_completed(&self) {
        let mut count = self.request_count.write().await;
        if *count > 0 {
            *count -= 1;
        }
        self.active_requests_gauge.set(*count as f64).await;
    }

    /// Get current connection count
    pub async fn get_connection_count(&self) -> u32 {
        *self.connection_count.read().await
    }

    /// Get current active request count
    pub async fn get_request_count(&self) -> u32 {
        *self.request_count.read().await
    }

    /// Get server uptime in seconds
    pub fn get_uptime(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }
}
