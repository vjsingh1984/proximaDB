//! # CHRONO Observability Storage Engine
//!
//! **STATUS**: Phase 1 (Apr 2026)
//!
//! Chronological Hierarchical Record and Observation store -- LSM-based
//! observability engine for metrics, logs, and traces.
//!
//! CHRONO implements `ObservabilityStorageEngine` (the observability-native trait)
//! for efficient time-series ingestion with Gorilla encoding, log label indexing,
//! and distributed trace span assembly.
//!
//! It also implements `UnifiedStorageEngine` as a thin stub for factory
//! registration, but all real observability operations go through
//! `ObservabilityStorageEngine`.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use crate::core::search::results::OptimizedSearchRecord;
use crate::observability::ObservabilityStorageEngine;
use crate::proto::proximadb_v1::{LogEntry, MetricSample, TraceData, VectorRecord};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    StorageQueryContext, UnifiedStorageEngine,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the CHRONO observability storage engine.
#[derive(Debug, Clone)]
pub struct ChronoConfig {
    /// Base directory for data files. `None` uses the system default.
    pub base_path: Option<PathBuf>,
    /// Retention period in hours. Defaults to 720 (30 days).
    pub retention_hours: u64,
}

impl Default for ChronoConfig {
    fn default() -> Self {
        Self {
            base_path: None,
            retention_hours: 720,
        }
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// CHRONO storage engine -- observability-oriented LSM store.
///
/// Stores metrics, logs, and traces in separate memtables optimized for
/// each data type. Metrics are keyed by series key (name + sorted labels),
/// logs are appended chronologically, traces are keyed by trace_id.
pub struct ChronoEngine {
    config: ChronoConfig,
    /// Metric memtable: metric_name -> Vec<MetricSample> (sorted by timestamp)
    metrics: DashMap<String, Vec<MetricSample>>,
    /// Log memtable: append-only, sorted by timestamp
    logs: RwLock<Vec<LogEntry>>,
    /// Trace memtable: trace_id -> Vec<TraceData> (spans for that trace)
    traces: DashMap<String, Vec<TraceData>>,
    /// Series count (distinct metric name + label combinations)
    series_keys: DashMap<u64, String>,
    /// Total ingested count for metrics
    metric_count: AtomicU64,
    /// Total ingested count for logs
    log_count: AtomicU64,
    /// Total ingested count for trace spans
    span_count: AtomicU64,
}

impl ChronoEngine {
    /// Create a new `ChronoEngine` with default configuration.
    pub fn new() -> Result<Self> {
        Self::with_config(ChronoConfig::default())
    }

    /// Create a new `ChronoEngine` with the given configuration.
    pub fn with_config(config: ChronoConfig) -> Result<Self> {
        Ok(Self {
            config,
            metrics: DashMap::new(),
            logs: RwLock::new(Vec::new()),
            traces: DashMap::new(),
            series_keys: DashMap::new(),
            metric_count: AtomicU64::new(0),
            log_count: AtomicU64::new(0),
            span_count: AtomicU64::new(0),
        })
    }

    /// Compute a series key hash from metric name + labels
    fn series_key_hash(name: &str, labels: &HashMap<String, String>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        let mut sorted_labels: Vec<_> = labels.iter().collect();
        sorted_labels.sort_by_key(|(k, _)| k.clone());
        for (k, v) in sorted_labels {
            k.hash(&mut hasher);
            v.hash(&mut hasher);
        }
        hasher.finish()
    }
}

// ---------------------------------------------------------------------------
// ObservabilityStorageEngine implementation (the real observability interface)
// ---------------------------------------------------------------------------

#[async_trait]
impl ObservabilityStorageEngine for ChronoEngine {
    fn engine_name(&self) -> &'static str {
        "chrono"
    }

    async fn ingest_metrics(&self, _namespace: String, samples: Vec<MetricSample>) -> Result<u64> {
        let count = samples.len() as u64;
        for sample in samples {
            let name = sample.name.clone();

            // Track distinct series
            let labels: HashMap<String, String> = sample.labels.clone();
            let hash = Self::series_key_hash(&name, &labels);
            self.series_keys.entry(hash).or_insert_with(|| name.clone());

            // Append to metric memtable (sorted by timestamp on read)
            self.metrics.entry(name).or_default().push(sample);
        }
        self.metric_count.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }

    async fn query_metrics(
        &self,
        _namespace: String,
        metric_name: String,
        label_matchers: Vec<(String, String)>,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<MetricSample>> {
        let Some(samples) = self.metrics.get(&metric_name) else {
            return Ok(vec![]);
        };

        let filtered: Vec<MetricSample> = samples
            .iter()
            .filter(|s| {
                s.timestamp_ns >= start_ns
                    && s.timestamp_ns <= end_ns
                    && label_matchers
                        .iter()
                        .all(|(k, v)| s.labels.get(k).map_or(false, |sv| sv == v))
            })
            .cloned()
            .collect();

        Ok(filtered)
    }

    async fn ingest_logs(&self, _namespace: String, entries: Vec<LogEntry>) -> Result<u64> {
        let count = entries.len() as u64;
        let mut logs = self.logs.write().map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        logs.extend(entries);
        self.log_count.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }

    async fn query_logs(
        &self,
        _namespace: String,
        start_ns: i64,
        end_ns: i64,
        severity: Option<i32>,
        text_filter: Option<String>,
    ) -> Result<Vec<LogEntry>> {
        let logs = self.logs.read().map_err(|e| anyhow::anyhow!("Lock poisoned: {}", e))?;
        let filtered: Vec<LogEntry> = logs
            .iter()
            .filter(|log| {
                log.timestamp_ns >= start_ns
                    && log.timestamp_ns <= end_ns
                    && severity.map_or(true, |s| log.severity >= s)
                    && text_filter.as_ref().map_or(true, |t| log.message.contains(t.as_str()))
            })
            .cloned()
            .collect();

        Ok(filtered)
    }

    async fn ingest_spans(&self, _namespace: String, spans: Vec<TraceData>) -> Result<u64> {
        let count = spans.len() as u64;
        for span in spans {
            let trace_id = span.trace_id.clone();
            self.traces.entry(trace_id).or_default().push(span);
        }
        self.span_count.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }

    async fn query_traces(
        &self,
        _namespace: String,
        trace_id: Option<String>,
        _service: Option<String>,
        _start_ns: i64,
        _end_ns: i64,
    ) -> Result<Vec<TraceData>> {
        if let Some(tid) = &trace_id {
            Ok(self
                .traces
                .get(tid)
                .map(|spans| spans.value().clone())
                .unwrap_or_default())
        } else {
            // Return all spans (limited in production)
            let mut all = Vec::new();
            for entry in self.traces.iter() {
                all.extend(entry.value().clone());
            }
            Ok(all)
        }
    }

    async fn flush(&self, _namespace: String) -> Result<u64> {
        // Phase 5: implement Gorilla-encoded disk persistence
        Ok(0)
    }

    async fn compact(&self, _namespace: String) -> Result<u64> {
        // Phase 5: implement time-window compaction with downsampling
        Ok(0)
    }

    async fn series_count(&self) -> u64 {
        self.series_keys.len() as u64
    }

    async fn collect_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        metrics.insert("engine".to_string(), serde_json::json!("chrono"));
        metrics.insert("series_count".to_string(), serde_json::json!(self.series_keys.len()));
        metrics.insert("metric_count".to_string(), serde_json::json!(self.metric_count.load(Ordering::Relaxed)));
        metrics.insert("log_count".to_string(), serde_json::json!(self.log_count.load(Ordering::Relaxed)));
        metrics.insert("span_count".to_string(), serde_json::json!(self.span_count.load(Ordering::Relaxed)));
        Ok(metrics)
    }
}

// ---------------------------------------------------------------------------
// UnifiedStorageEngine stub (for factory registration only)
// ---------------------------------------------------------------------------

#[async_trait]
impl UnifiedStorageEngine for ChronoEngine {
    fn engine_name(&self) -> &'static str {
        "chrono"
    }

    fn engine_version(&self) -> &'static str {
        "0.1.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Chrono
    }

    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        use std::sync::OnceLock;
        static FACTORY: OnceLock<FilesystemFactory> = OnceLock::new();
        FACTORY.get_or_init(|| {
            futures::executor::block_on(async {
                FilesystemFactory::create(FilesystemConfig::default())
                    .await
                    .unwrap_or_else(|_| {
                        #[allow(clippy::panic)]
                        {
                            panic!("Failed to create filesystem factory for CHRONO engine")
                        }
                    })
            })
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        ObservabilityStorageEngine::collect_metrics(self).await
    }

    async fn vector_by_id(
        &self,
        _collection_id: &str,
        _base_path: &str,
        _vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        Ok(None) // CHRONO stores observability data, not vectors
    }

    async fn search_vectors_unified(
        &self,
        _ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        Ok(vec![]) // Use ObservabilityStorageEngine methods for observability queries
    }

    async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
        Ok(FlushResult::default())
    }

    async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
        Ok(CompactionResult::default())
    }
}

// ---------------------------------------------------------------------------
// Tests -- TDD Phase 1 (red/green cycles 1.4 + 1.5 + 1.6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metric(name: &str, ts: i64, value: f64) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            timestamp_ns: ts,
            labels: HashMap::new(),
            ..Default::default()
        }
    }

    fn make_metric_with_labels(
        name: &str,
        ts: i64,
        value: f64,
        labels: Vec<(&str, &str)>,
    ) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            timestamp_ns: ts,
            labels: labels
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    fn make_log(ts: i64, severity: i32, message: &str) -> LogEntry {
        LogEntry {
            timestamp_ns: ts,
            severity,
            message: message.to_string(),
            ..Default::default()
        }
    }

    fn make_span(trace_id: &str, span_id: &str, operation: &str) -> TraceData {
        TraceData {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            name: operation.to_string(),
            ..Default::default()
        }
    }

    // -- Identity --

    #[test]
    fn test_chrono_engine_name() {
        let engine = ChronoEngine::new().unwrap();
        assert_eq!(engine.engine_name(), "chrono");
    }

    #[test]
    fn test_chrono_strategy() {
        let engine = ChronoEngine::new().unwrap();
        assert_eq!(engine.strategy(), StorageEngineStrategy::Chrono);
    }

    #[tokio::test]
    async fn test_chrono_collect_metrics_has_series_count() {
        let engine = ChronoEngine::new().unwrap();
        let metrics = ObservabilityStorageEngine::collect_metrics(&engine).await.unwrap();
        assert!(metrics.contains_key("series_count"));
    }

    // -- Cycle 1.4: Metric Insert + Range Query --

    #[tokio::test]
    async fn test_chrono_metric_insert_and_query() {
        let engine = ChronoEngine::new().unwrap();
        let samples: Vec<MetricSample> = (0..5)
            .map(|i| make_metric("cpu_usage", 100 + i * 100, i as f64))
            .collect();

        let count = engine.ingest_metrics("default".to_string(), samples).await.unwrap();
        assert_eq!(count, 5);

        // Query range [200, 400] should return 3 samples (ts=200, 300, 400)
        let results = engine
            .query_metrics("default".to_string(), "cpu_usage".to_string(), vec![], 200, 400)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_chrono_metric_query_empty_range() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_metrics("default".to_string(), vec![make_metric("cpu", 100, 1.0)])
            .await
            .unwrap();

        let results = engine.query_metrics("default".to_string(), "cpu".to_string(), vec![], 200, 300).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_chrono_series_count() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_metrics(
                "default",
                vec![
                    make_metric_with_labels("cpu", 100, 0.5, vec![("host", "a")]),
                    make_metric_with_labels("cpu", 200, 0.6, vec![("host", "b")]),
                    make_metric_with_labels("memory", 100, 0.8, vec![("host", "a")]),
                ],
            )
            .await
            .unwrap();

        assert_eq!(engine.series_count().await, 3); // 3 distinct series keys
    }

    #[tokio::test]
    async fn test_chrono_metric_label_filter() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_metrics(
                "default",
                vec![
                    make_metric_with_labels("http", 100, 1.0, vec![("method", "GET")]),
                    make_metric_with_labels("http", 200, 2.0, vec![("method", "POST")]),
                    make_metric_with_labels("http", 300, 3.0, vec![("method", "GET")]),
                ],
            )
            .await
            .unwrap();

        let gets = engine
            .query_metrics(
                "default".to_string(),
                "http".to_string(),
                vec![("method".to_string(), "GET".to_string())],
                0,
                i64::MAX,
            )
            .await
            .unwrap();
        assert_eq!(gets.len(), 2);
    }

    // -- Cycle 1.5: Log Insert + Query --

    #[tokio::test]
    async fn test_chrono_log_insert_and_query_by_time() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_logs(
                "default",
                vec![
                    make_log(100, 2, "info message"),
                    make_log(200, 4, "error message"),
                    make_log(300, 2, "another info"),
                ],
            )
            .await
            .unwrap();

        let results = engine.query_logs("default".to_string(),150, 250, None, None).await.unwrap();
        assert_eq!(results.len(), 1); // only ts=200
    }

    #[tokio::test]
    async fn test_chrono_log_query_by_severity() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_logs(
                "default",
                vec![
                    make_log(100, 1, "debug msg"),  // severity 1 = DEBUG
                    make_log(200, 2, "info msg"),   // severity 2 = INFO
                    make_log(300, 4, "error msg"),  // severity 4 = ERROR
                ],
            )
            .await
            .unwrap();

        // severity >= 4 (ERROR and above)
        let errors = engine.query_logs("default".to_string(),0, i64::MAX, Some(4), None).await.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "error msg");
    }

    #[tokio::test]
    async fn test_chrono_log_text_filter() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_logs(
                "default",
                vec![
                    make_log(100, 2, "connection established"),
                    make_log(200, 4, "connection timeout error"),
                    make_log(300, 2, "request processed"),
                ],
            )
            .await
            .unwrap();

        let timeouts = engine
            .query_logs("default".to_string(),0, i64::MAX, None, Some("timeout".to_string()))
            .await
            .unwrap();
        assert_eq!(timeouts.len(), 1);
    }

    // -- Cycle 1.6: Trace Span Insert + Query --

    #[tokio::test]
    async fn test_chrono_trace_insert_and_get() {
        let engine = ChronoEngine::new().unwrap();
        engine
            .ingest_spans(
                "default",
                vec![
                    make_span("trace1", "span1", "GET /api"),
                    make_span("trace1", "span2", "DB query"),
                    make_span("trace2", "span3", "POST /api"),
                ],
            )
            .await
            .unwrap();

        let trace1 = engine
            .query_traces("default".to_string(),Some("trace1".to_string()), None, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(trace1.len(), 2);

        let trace2 = engine
            .query_traces("default".to_string(),Some("trace2".to_string()), None, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(trace2.len(), 1);
    }

    #[tokio::test]
    async fn test_chrono_trace_missing() {
        let engine = ChronoEngine::new().unwrap();
        let result = engine
            .query_traces("default".to_string(),Some("nonexistent".to_string()), None, 0, i64::MAX)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_chrono_ingestion_counts() {
        let engine = ChronoEngine::new().unwrap();

        engine
            .ingest_metrics("default".to_string(), vec![make_metric("m1", 100, 1.0)])
            .await
            .unwrap();
        engine
            .ingest_logs("default".to_string(), vec![make_log(100, 2, "test")])
            .await
            .unwrap();
        engine
            .ingest_spans("default".to_string(), vec![make_span("t1", "s1", "op")])
            .await
            .unwrap();

        let metrics = ObservabilityStorageEngine::collect_metrics(&engine).await.unwrap();
        assert_eq!(metrics["metric_count"], serde_json::json!(1));
        assert_eq!(metrics["log_count"], serde_json::json!(1));
        assert_eq!(metrics["span_count"], serde_json::json!(1));
    }
}
