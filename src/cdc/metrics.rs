/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! CDC metrics collection and reporting
//!
//! This module provides metrics for monitoring CDC pipeline health and performance.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// CDC metrics collector
#[derive(Debug)]
pub struct CdcMetrics {
    /// Source metrics by name
    sources: Arc<RwLock<HashMap<String, SourceMetrics>>>,
    /// Sink metrics by name
    sinks: Arc<RwLock<HashMap<String, SinkMetrics>>>,
    /// Transform metrics by name
    transforms: Arc<RwLock<HashMap<String, TransformMetrics>>>,
    /// Coordinator metrics
    coordinator: CoordinatorMetrics,
    /// Start time
    start_time: Instant,
}

impl CdcMetrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            sources: Arc::new(RwLock::new(HashMap::new())),
            sinks: Arc::new(RwLock::new(HashMap::new())),
            transforms: Arc::new(RwLock::new(HashMap::new())),
            coordinator: CoordinatorMetrics::new(),
            start_time: Instant::now(),
        }
    }

    /// Register a source for metrics collection
    pub async fn register_source(&self, name: &str) {
        let mut sources = self.sources.write().await;
        sources.insert(name.to_string(), SourceMetrics::new(name));
    }

    /// Register a sink for metrics collection
    pub async fn register_sink(&self, name: &str) {
        let mut sinks = self.sinks.write().await;
        sinks.insert(name.to_string(), SinkMetrics::new(name));
    }

    /// Register a transform for metrics collection
    pub async fn register_transform(&self, name: &str) {
        let mut transforms = self.transforms.write().await;
        transforms.insert(name.to_string(), TransformMetrics::new(name));
    }

    /// Record events received from a source
    pub async fn record_source_events(&self, source: &str, count: u64) {
        if let Some(metrics) = self.sources.read().await.get(source) {
            metrics.events_received.fetch_add(count, Ordering::Relaxed);
        }
    }

    /// Record bytes received from a source
    pub async fn record_source_bytes(&self, source: &str, bytes: u64) {
        if let Some(metrics) = self.sources.read().await.get(source) {
            metrics.bytes_received.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Record source error
    pub async fn record_source_error(&self, source: &str) {
        if let Some(metrics) = self.sources.read().await.get(source) {
            metrics.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Update source lag (milliseconds behind)
    pub async fn update_source_lag(&self, source: &str, lag_ms: u64) {
        if let Some(metrics) = self.sources.read().await.get(source) {
            metrics.lag_ms.store(lag_ms, Ordering::Relaxed);
        }
    }

    /// Record events sent to a sink
    pub async fn record_sink_events(&self, sink: &str, count: u64) {
        if let Some(metrics) = self.sinks.read().await.get(sink) {
            metrics.events_sent.fetch_add(count, Ordering::Relaxed);
        }
    }

    /// Record bytes sent to a sink
    pub async fn record_sink_bytes(&self, sink: &str, bytes: u64) {
        if let Some(metrics) = self.sinks.read().await.get(sink) {
            metrics.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Record sink error
    pub async fn record_sink_error(&self, sink: &str) {
        if let Some(metrics) = self.sinks.read().await.get(sink) {
            metrics.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record sink retry
    pub async fn record_sink_retry(&self, sink: &str) {
        if let Some(metrics) = self.sinks.read().await.get(sink) {
            metrics.retries.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record transform processing
    pub async fn record_transform(&self, transform: &str, count: u64, duration: Duration) {
        if let Some(metrics) = self.transforms.read().await.get(transform) {
            metrics.events_processed.fetch_add(count, Ordering::Relaxed);
            metrics
                .processing_time_us
                .fetch_add(duration.as_micros() as u64, Ordering::Relaxed);
        }
    }

    /// Record transform error
    pub async fn record_transform_error(&self, transform: &str) {
        if let Some(metrics) = self.transforms.read().await.get(transform) {
            metrics.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record coordinator event processed
    pub fn record_event_processed(&self) {
        self.coordinator
            .total_events
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record coordinator event dropped
    pub fn record_event_dropped(&self) {
        self.coordinator
            .dropped_events
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get coordinator metrics
    pub fn coordinator(&self) -> &CoordinatorMetrics {
        &self.coordinator
    }

    /// Get uptime
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get all source metrics
    pub async fn source_metrics(&self) -> HashMap<String, SourceMetricsSnapshot> {
        let sources = self.sources.read().await;
        sources
            .iter()
            .map(|(name, m)| (name.clone(), m.snapshot()))
            .collect()
    }

    /// Get all sink metrics
    pub async fn sink_metrics(&self) -> HashMap<String, SinkMetricsSnapshot> {
        let sinks = self.sinks.read().await;
        sinks
            .iter()
            .map(|(name, m)| (name.clone(), m.snapshot()))
            .collect()
    }

    /// Get all transform metrics
    pub async fn transform_metrics(&self) -> HashMap<String, TransformMetricsSnapshot> {
        let transforms = self.transforms.read().await;
        transforms
            .iter()
            .map(|(name, m)| (name.clone(), m.snapshot()))
            .collect()
    }

    /// Get complete metrics snapshot
    pub async fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime: self.uptime(),
            total_events: self.coordinator.total_events.load(Ordering::Relaxed),
            dropped_events: self.coordinator.dropped_events.load(Ordering::Relaxed),
            sources: self.source_metrics().await,
            sinks: self.sink_metrics().await,
            transforms: self.transform_metrics().await,
        }
    }
}

impl Default for CdcMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Source metrics
#[derive(Debug)]
pub struct SourceMetrics {
    /// Source name
    #[allow(dead_code)]
    name: String,
    /// Events received
    events_received: AtomicU64,
    /// Bytes received
    bytes_received: AtomicU64,
    /// Errors
    errors: AtomicU64,
    /// Lag in milliseconds
    lag_ms: AtomicU64,
}

impl SourceMetrics {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            events_received: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            lag_ms: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> SourceMetricsSnapshot {
        SourceMetricsSnapshot {
            events_received: self.events_received.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            lag_ms: self.lag_ms.load(Ordering::Relaxed),
        }
    }
}

/// Source metrics snapshot
#[derive(Debug, Clone)]
pub struct SourceMetricsSnapshot {
    pub events_received: u64,
    pub bytes_received: u64,
    pub errors: u64,
    pub lag_ms: u64,
}

/// Sink metrics
#[derive(Debug)]
pub struct SinkMetrics {
    /// Sink name
    #[allow(dead_code)]
    name: String,
    /// Events sent
    events_sent: AtomicU64,
    /// Bytes sent
    bytes_sent: AtomicU64,
    /// Errors
    errors: AtomicU64,
    /// Retries
    retries: AtomicU64,
}

impl SinkMetrics {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            events_sent: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            retries: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> SinkMetricsSnapshot {
        SinkMetricsSnapshot {
            events_sent: self.events_sent.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
        }
    }
}

/// Sink metrics snapshot
#[derive(Debug, Clone)]
pub struct SinkMetricsSnapshot {
    pub events_sent: u64,
    pub bytes_sent: u64,
    pub errors: u64,
    pub retries: u64,
}

/// Transform metrics
#[derive(Debug)]
pub struct TransformMetrics {
    /// Transform name
    #[allow(dead_code)]
    name: String,
    /// Events processed
    events_processed: AtomicU64,
    /// Processing time in microseconds
    processing_time_us: AtomicU64,
    /// Errors
    errors: AtomicU64,
}

impl TransformMetrics {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            events_processed: AtomicU64::new(0),
            processing_time_us: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> TransformMetricsSnapshot {
        let events = self.events_processed.load(Ordering::Relaxed);
        let time_us = self.processing_time_us.load(Ordering::Relaxed);
        TransformMetricsSnapshot {
            events_processed: events,
            processing_time_us: time_us,
            avg_latency_us: if events > 0 { time_us / events } else { 0 },
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

/// Transform metrics snapshot
#[derive(Debug, Clone)]
pub struct TransformMetricsSnapshot {
    pub events_processed: u64,
    pub processing_time_us: u64,
    pub avg_latency_us: u64,
    pub errors: u64,
}

/// Coordinator metrics
#[derive(Debug)]
pub struct CoordinatorMetrics {
    /// Total events processed
    total_events: AtomicU64,
    /// Events dropped
    dropped_events: AtomicU64,
}

impl CoordinatorMetrics {
    fn new() -> Self {
        Self {
            total_events: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
        }
    }

    /// Get total events
    pub fn total_events(&self) -> u64 {
        self.total_events.load(Ordering::Relaxed)
    }

    /// Get dropped events
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
}

/// Complete metrics snapshot
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub uptime: Duration,
    pub total_events: u64,
    pub dropped_events: u64,
    pub sources: HashMap<String, SourceMetricsSnapshot>,
    pub sinks: HashMap<String, SinkMetricsSnapshot>,
    pub transforms: HashMap<String, TransformMetricsSnapshot>,
}

impl MetricsSnapshot {
    /// Calculate total events/second throughput
    pub fn events_per_second(&self) -> f64 {
        if self.uptime.as_secs() > 0 {
            self.total_events as f64 / self.uptime.as_secs_f64()
        } else {
            0.0
        }
    }

    /// Calculate drop rate
    pub fn drop_rate(&self) -> f64 {
        let total = self.total_events + self.dropped_events;
        if total > 0 {
            self.dropped_events as f64 / total as f64
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_creation() {
        let metrics = CdcMetrics::new();
        assert!(metrics.uptime().as_millis() >= 0);
    }

    #[tokio::test]
    async fn test_source_metrics() {
        let metrics = CdcMetrics::new();
        metrics.register_source("pg_source").await;

        metrics.record_source_events("pg_source", 100).await;
        metrics.record_source_bytes("pg_source", 50_000).await;
        metrics.record_source_error("pg_source").await;
        metrics.update_source_lag("pg_source", 1500).await;

        let snapshot = metrics.source_metrics().await;
        let pg_metrics = snapshot.get("pg_source").unwrap();

        assert_eq!(pg_metrics.events_received, 100);
        assert_eq!(pg_metrics.bytes_received, 50_000);
        assert_eq!(pg_metrics.errors, 1);
        assert_eq!(pg_metrics.lag_ms, 1500);
    }

    #[tokio::test]
    async fn test_sink_metrics() {
        let metrics = CdcMetrics::new();
        metrics.register_sink("kafka_sink").await;

        metrics.record_sink_events("kafka_sink", 50).await;
        metrics.record_sink_bytes("kafka_sink", 25_000).await;
        metrics.record_sink_retry("kafka_sink").await;
        metrics.record_sink_error("kafka_sink").await;

        let snapshot = metrics.sink_metrics().await;
        let kafka_metrics = snapshot.get("kafka_sink").unwrap();

        assert_eq!(kafka_metrics.events_sent, 50);
        assert_eq!(kafka_metrics.bytes_sent, 25_000);
        assert_eq!(kafka_metrics.retries, 1);
        assert_eq!(kafka_metrics.errors, 1);
    }

    #[tokio::test]
    async fn test_transform_metrics() {
        let metrics = CdcMetrics::new();
        metrics.register_transform("vectorize").await;

        metrics
            .record_transform("vectorize", 100, Duration::from_millis(500))
            .await;

        let snapshot = metrics.transform_metrics().await;
        let transform = snapshot.get("vectorize").unwrap();

        assert_eq!(transform.events_processed, 100);
        assert!(transform.processing_time_us >= 500_000); // 500ms = 500000us
        assert!(transform.avg_latency_us >= 5000); // ~5000us per event
    }

    #[tokio::test]
    async fn test_coordinator_metrics() {
        let metrics = CdcMetrics::new();

        for _ in 0..100 {
            metrics.record_event_processed();
        }
        for _ in 0..5 {
            metrics.record_event_dropped();
        }

        assert_eq!(metrics.coordinator().total_events(), 100);
        assert_eq!(metrics.coordinator().dropped_events(), 5);
    }

    #[tokio::test]
    async fn test_full_snapshot() {
        let metrics = CdcMetrics::new();
        metrics.register_source("src").await;
        metrics.register_sink("sink").await;

        metrics.record_source_events("src", 1000).await;
        metrics.record_sink_events("sink", 950).await;

        for _ in 0..1000 {
            metrics.record_event_processed();
        }

        let snapshot = metrics.snapshot().await;
        assert_eq!(snapshot.total_events, 1000);
        assert!(snapshot.events_per_second() >= 0.0);
    }

    #[test]
    fn test_snapshot_calculations() {
        let snapshot = MetricsSnapshot {
            uptime: Duration::from_secs(100),
            total_events: 10_000,
            dropped_events: 100,
            sources: HashMap::new(),
            sinks: HashMap::new(),
            transforms: HashMap::new(),
        };

        assert_eq!(snapshot.events_per_second(), 100.0);
        assert!((snapshot.drop_rate() - 0.0099).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_unregistered_source() {
        let metrics = CdcMetrics::new();

        // Should not panic for unregistered source
        metrics.record_source_events("unknown", 100).await;
        metrics.record_source_bytes("unknown", 1000).await;

        let snapshot = metrics.source_metrics().await;
        assert!(snapshot.get("unknown").is_none());
    }
}
