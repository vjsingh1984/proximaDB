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

//! Prometheus metrics for streaming operations
//!
//! This module provides comprehensive metrics collection for monitoring
//! streaming performance, throughput, and backpressure.

use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, Opts, Registry,
};
use std::sync::Arc;

/// Streaming metrics collector
#[derive(Clone)]
pub struct StreamMetrics {
    /// Number of active streaming sessions
    pub active_streams: Gauge,

    /// Total records received across all streams
    pub records_received_total: Counter,

    /// Total records processed (flushed to storage)
    pub records_processed_total: Counter,

    /// Total records dropped due to backpressure
    pub records_dropped_total: Counter,

    /// Records received by collection
    pub records_by_collection: CounterVec,

    /// Current buffer utilization (0.0 - 1.0)
    pub buffer_utilization: GaugeVec,

    /// Backpressure events by level
    pub backpressure_events: CounterVec,

    /// Ingestion latency histogram (seconds)
    pub ingestion_latency: Histogram,

    /// Ingestion latency by collection
    pub ingestion_latency_by_collection: HistogramVec,

    /// Flush latency histogram (seconds)
    pub flush_latency: Histogram,

    /// Records per batch histogram
    pub batch_size: Histogram,

    /// Rate limiter rejections
    pub rate_limit_rejections: Counter,

    /// Session creation count
    pub sessions_created_total: Counter,

    /// Session close count
    pub sessions_closed_total: Counter,

    /// Session timeout count
    pub sessions_timed_out_total: Counter,

    /// Flush success count
    pub flush_success_total: Counter,

    /// Flush failure count
    pub flush_failure_total: Counter,

    /// Flush retry count
    pub flush_retries_total: Counter,

    /// Records flushed to storage
    pub records_flushed_total: Counter,

    /// Bytes flushed to storage
    pub bytes_flushed_total: Counter,

    /// Flush operations by collection
    pub flush_by_collection: CounterVec,
}

impl StreamMetrics {
    /// Create a new StreamMetrics instance
    pub fn new() -> Self {
        Self {
            active_streams: Gauge::new(
                "proximadb_stream_active_count",
                "Number of active streaming sessions",
            )
            .unwrap(),

            records_received_total: Counter::new(
                "proximadb_stream_records_received_total",
                "Total records received via streaming",
            )
            .unwrap(),

            records_processed_total: Counter::new(
                "proximadb_stream_records_processed_total",
                "Total records processed and flushed to storage",
            )
            .unwrap(),

            records_dropped_total: Counter::new(
                "proximadb_stream_records_dropped_total",
                "Total records dropped due to backpressure",
            )
            .unwrap(),

            records_by_collection: CounterVec::new(
                Opts::new(
                    "proximadb_stream_records_by_collection_total",
                    "Records received by collection",
                ),
                &["collection"],
            )
            .unwrap(),

            buffer_utilization: GaugeVec::new(
                Opts::new(
                    "proximadb_stream_buffer_utilization",
                    "Buffer utilization ratio (0.0 - 1.0)",
                ),
                &["stream_id"],
            )
            .unwrap(),

            backpressure_events: CounterVec::new(
                Opts::new(
                    "proximadb_stream_backpressure_events_total",
                    "Backpressure events by level",
                ),
                &["level"],
            )
            .unwrap(),

            ingestion_latency: Histogram::with_opts(
                HistogramOpts::new(
                    "proximadb_stream_ingestion_latency_seconds",
                    "Time from record receipt to buffer insertion",
                )
                .buckets(vec![
                    0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0,
                ]),
            )
            .unwrap(),

            ingestion_latency_by_collection: HistogramVec::new(
                HistogramOpts::new(
                    "proximadb_stream_ingestion_latency_by_collection_seconds",
                    "Ingestion latency by collection",
                )
                .buckets(vec![
                    0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0,
                ]),
                &["collection"],
            )
            .unwrap(),

            flush_latency: Histogram::with_opts(
                HistogramOpts::new(
                    "proximadb_stream_flush_latency_seconds",
                    "Time to flush buffer to storage",
                )
                .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]),
            )
            .unwrap(),

            batch_size: Histogram::with_opts(
                HistogramOpts::new("proximadb_stream_batch_size", "Number of records per batch")
                    .buckets(vec![1.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0]),
            )
            .unwrap(),

            rate_limit_rejections: Counter::new(
                "proximadb_stream_rate_limit_rejections_total",
                "Total rate limit rejections",
            )
            .unwrap(),

            sessions_created_total: Counter::new(
                "proximadb_stream_sessions_created_total",
                "Total streaming sessions created",
            )
            .unwrap(),

            sessions_closed_total: Counter::new(
                "proximadb_stream_sessions_closed_total",
                "Total streaming sessions closed normally",
            )
            .unwrap(),

            sessions_timed_out_total: Counter::new(
                "proximadb_stream_sessions_timed_out_total",
                "Total streaming sessions timed out",
            )
            .unwrap(),

            flush_success_total: Counter::new(
                "proximadb_stream_flush_success_total",
                "Total successful flush operations",
            )
            .unwrap(),

            flush_failure_total: Counter::new(
                "proximadb_stream_flush_failure_total",
                "Total failed flush operations",
            )
            .unwrap(),

            flush_retries_total: Counter::new(
                "proximadb_stream_flush_retries_total",
                "Total flush retry attempts",
            )
            .unwrap(),

            records_flushed_total: Counter::new(
                "proximadb_stream_records_flushed_total",
                "Total records flushed to storage",
            )
            .unwrap(),

            bytes_flushed_total: Counter::new(
                "proximadb_stream_bytes_flushed_total",
                "Total bytes flushed to storage",
            )
            .unwrap(),

            flush_by_collection: CounterVec::new(
                Opts::new(
                    "proximadb_stream_flush_by_collection_total",
                    "Flush operations by collection",
                ),
                &["collection", "status"],
            )
            .unwrap(),
        }
    }

    /// Register all metrics with a Prometheus registry
    pub fn register(&self, registry: &Registry) -> Result<(), prometheus::Error> {
        registry.register(Box::new(self.active_streams.clone()))?;
        registry.register(Box::new(self.records_received_total.clone()))?;
        registry.register(Box::new(self.records_processed_total.clone()))?;
        registry.register(Box::new(self.records_dropped_total.clone()))?;
        registry.register(Box::new(self.records_by_collection.clone()))?;
        registry.register(Box::new(self.buffer_utilization.clone()))?;
        registry.register(Box::new(self.backpressure_events.clone()))?;
        registry.register(Box::new(self.ingestion_latency.clone()))?;
        registry.register(Box::new(self.ingestion_latency_by_collection.clone()))?;
        registry.register(Box::new(self.flush_latency.clone()))?;
        registry.register(Box::new(self.batch_size.clone()))?;
        registry.register(Box::new(self.rate_limit_rejections.clone()))?;
        registry.register(Box::new(self.sessions_created_total.clone()))?;
        registry.register(Box::new(self.sessions_closed_total.clone()))?;
        registry.register(Box::new(self.sessions_timed_out_total.clone()))?;
        registry.register(Box::new(self.flush_success_total.clone()))?;
        registry.register(Box::new(self.flush_failure_total.clone()))?;
        registry.register(Box::new(self.flush_retries_total.clone()))?;
        registry.register(Box::new(self.records_flushed_total.clone()))?;
        registry.register(Box::new(self.bytes_flushed_total.clone()))?;
        registry.register(Box::new(self.flush_by_collection.clone()))?;
        Ok(())
    }

    /// Record a backpressure event
    pub fn record_backpressure(&self, level: super::BackpressureLevel) {
        let label = match level {
            super::BackpressureLevel::None => "none",
            super::BackpressureLevel::Low => "low",
            super::BackpressureLevel::Medium => "medium",
            super::BackpressureLevel::High => "high",
            super::BackpressureLevel::Critical => "critical",
        };
        self.backpressure_events.with_label_values(&[label]).inc();
    }

    /// Record ingestion latency
    pub fn record_ingestion_latency(&self, collection: &str, latency_secs: f64) {
        self.ingestion_latency.observe(latency_secs);
        self.ingestion_latency_by_collection
            .with_label_values(&[collection])
            .observe(latency_secs);
    }

    /// Update buffer utilization for a stream
    pub fn set_buffer_utilization(&self, stream_id: &str, utilization: f64) {
        self.buffer_utilization
            .with_label_values(&[stream_id])
            .set(utilization);
    }

    /// Increment active streams
    pub fn inc_active_streams(&self) {
        self.active_streams.inc();
    }

    /// Decrement active streams
    pub fn dec_active_streams(&self) {
        self.active_streams.dec();
    }

    /// Record records received
    pub fn record_received(&self, collection: &str, count: u64) {
        self.records_received_total.inc_by(count as f64);
        self.records_by_collection
            .with_label_values(&[collection])
            .inc_by(count as f64);
    }

    /// Record records processed
    pub fn record_processed(&self, count: u64) {
        self.records_processed_total.inc_by(count as f64);
    }

    /// Record records dropped
    pub fn record_dropped(&self, count: u64) {
        self.records_dropped_total.inc_by(count as f64);
    }

    /// Record a successful flush operation
    pub fn record_flush_success(
        &self,
        collection: &str,
        records: usize,
        bytes: usize,
        retries: u32,
    ) {
        self.flush_success_total.inc();
        self.records_flushed_total.inc_by(records as f64);
        self.bytes_flushed_total.inc_by(bytes as f64);
        self.flush_by_collection
            .with_label_values(&[collection, "success"])
            .inc();
        if retries > 0 {
            self.flush_retries_total.inc_by(retries as f64);
        }
    }

    /// Record a failed flush operation
    pub fn record_flush_failure(&self, collection: &str, retries: u32) {
        self.flush_failure_total.inc();
        self.flush_by_collection
            .with_label_values(&[collection, "failure"])
            .inc();
        if retries > 0 {
            self.flush_retries_total.inc_by(retries as f64);
        }
    }
}

impl Default for StreamMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Global streaming metrics instance
static STREAM_METRICS: std::sync::OnceLock<Arc<StreamMetrics>> = std::sync::OnceLock::new();

/// Get or initialize the global streaming metrics
pub fn get_or_init_metrics() -> Arc<StreamMetrics> {
    STREAM_METRICS
        .get_or_init(|| Arc::new(StreamMetrics::new()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = StreamMetrics::new();
        metrics.inc_active_streams();
        metrics.record_received("test_collection", 100);
        metrics.record_processed(50);
        metrics.record_backpressure(super::super::BackpressureLevel::High);
    }

    #[test]
    fn test_global_metrics() {
        let m1 = get_or_init_metrics();
        let m2 = get_or_init_metrics();
        // Should be the same instance
        assert!(Arc::ptr_eq(&m1, &m2));
    }
}
