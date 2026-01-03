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

//! Document Store Metrics Collector
//!
//! Integrates with ProximaDB's unified metrics framework to collect
//! document-specific performance and usage metrics.
//!
//! ## Metrics Collected
//!
//! ### Operation Metrics
//! - Document insert/update/delete operation counts and latencies
//! - Batch operation statistics
//! - Query counts and latencies
//!
//! ### Collection Metrics
//! - Collection creation/deletion counts
//! - Document counts per collection
//! - Storage size per collection
//!
//! ### Tiering Metrics
//! - Hot tier document counts (in-memory)
//! - Cold tier document counts (flushed to storage)
//!
//! ### Index Metrics
//! - Full-text index size (if Tantivy is used)
//! - Path index statistics
//! - Array index statistics

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::{MetricsCollector, MetricsSample};

/// Metrics collector for document store operations
pub struct DocumentMetricsCollector {
    name: &'static str,
    last_sample: Arc<RwLock<Option<DocumentMetricsSample>>>,
    accumulated_metrics: Arc<RwLock<DocumentMetricsAccumulator>>,
}

/// Document-specific metrics sample
#[derive(Debug, Clone)]
struct DocumentMetricsSample {
    timestamp: Instant,

    // Collection metrics
    total_collections: u64,
    total_documents: u64,
    total_storage_bytes: u64,

    // Operation counts
    insert_count: u64,
    update_count: u64,
    delete_count: u64,
    query_count: u64,
    batch_insert_count: u64,

    // Operation latencies (in microseconds)
    avg_insert_latency_us: f64,
    avg_update_latency_us: f64,
    avg_delete_latency_us: f64,
    avg_query_latency_us: f64,

    // Error counts
    insert_errors: u64,
    update_errors: u64,
    delete_errors: u64,
    query_errors: u64,

    // Tiering metrics
    hot_tier_documents: u64,
    cold_tier_documents: u64,
    hot_tier_bytes: u64,
    cold_tier_bytes: u64,

    // Index metrics
    fulltext_index_size_bytes: u64,
    path_indexes_count: u64,
    array_indexes_count: u64,

    // Collection lifecycle
    collections_created: u64,
    collections_deleted: u64,
}

/// Accumulator for tracking operation metrics over time
#[derive(Debug, Default, Clone)]
struct DocumentMetricsAccumulator {
    // Operation counters
    insert_count: u64,
    update_count: u64,
    delete_count: u64,
    query_count: u64,
    batch_insert_count: u64,

    // Latency accumulators (in microseconds)
    insert_total_latency_us: f64,
    update_total_latency_us: f64,
    delete_total_latency_us: f64,
    query_total_latency_us: f64,

    // Error counters
    insert_errors: u64,
    update_errors: u64,
    delete_errors: u64,
    query_errors: u64,

    // Collection lifecycle counters
    collections_created: u64,
    collections_deleted: u64,

    // Last reset timestamp
    last_reset: Option<Instant>,
}

impl DocumentMetricsCollector {
    /// Create new document metrics collector
    pub fn new() -> Self {
        Self {
            name: "document_store",
            last_sample: Arc::new(RwLock::new(None)),
            accumulated_metrics: Arc::new(RwLock::new(DocumentMetricsAccumulator::default())),
        }
    }

    /// Record a document insert operation
    pub async fn record_insert(&self, latency_us: f64, error: bool) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.insert_count += 1;
        acc.insert_total_latency_us += latency_us;
        if error {
            acc.insert_errors += 1;
        }
    }

    /// Record a document update operation
    pub async fn record_update(&self, latency_us: f64, error: bool) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.update_count += 1;
        acc.update_total_latency_us += latency_us;
        if error {
            acc.update_errors += 1;
        }
    }

    /// Record a document delete operation
    pub async fn record_delete(&self, latency_us: f64, error: bool) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.delete_count += 1;
        acc.delete_total_latency_us += latency_us;
        if error {
            acc.delete_errors += 1;
        }
    }

    /// Record a document query operation
    pub async fn record_query(&self, latency_us: f64, error: bool) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.query_count += 1;
        acc.query_total_latency_us += latency_us;
        if error {
            acc.query_errors += 1;
        }
    }

    /// Record a batch insert operation
    pub async fn record_batch_insert(&self, document_count: u64, latency_us: f64, error: bool) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.batch_insert_count += 1;
        acc.insert_count += document_count;
        acc.insert_total_latency_us += latency_us;
        if error {
            acc.insert_errors += 1;
        }
    }

    /// Record a collection creation
    pub async fn record_collection_created(&self) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.collections_created += 1;
    }

    /// Record a collection deletion
    pub async fn record_collection_deleted(&self) {
        let mut acc = self.accumulated_metrics.write().await;
        acc.collections_deleted += 1;
    }

    /// Collect document-specific metrics
    async fn collect_document_metrics(&self) -> Result<DocumentMetricsSample> {
        let timestamp = Instant::now();
        let acc = self.accumulated_metrics.read().await;

        // Calculate average latencies
        let avg_insert_latency_us = if acc.insert_count > 0 {
            acc.insert_total_latency_us / acc.insert_count as f64
        } else {
            0.0
        };

        let avg_update_latency_us = if acc.update_count > 0 {
            acc.update_total_latency_us / acc.update_count as f64
        } else {
            0.0
        };

        let avg_delete_latency_us = if acc.delete_count > 0 {
            acc.delete_total_latency_us / acc.delete_count as f64
        } else {
            0.0
        };

        let avg_query_latency_us = if acc.query_count > 0 {
            acc.query_total_latency_us / acc.query_count as f64
        } else {
            0.0
        };

        // For MVP: Use estimated values for metrics that require DocumentService integration
        // In production, these would be populated from the actual DocumentService
        let (total_collections, total_documents, total_storage_bytes) =
            self.estimate_collection_stats().await;
        let (hot_tier_documents, cold_tier_documents, hot_tier_bytes, cold_tier_bytes) =
            self.estimate_tiering_stats().await;
        let (fulltext_index_size_bytes, path_indexes_count, array_indexes_count) =
            self.estimate_index_stats().await;

        Ok(DocumentMetricsSample {
            timestamp,
            total_collections,
            total_documents,
            total_storage_bytes,
            insert_count: acc.insert_count,
            update_count: acc.update_count,
            delete_count: acc.delete_count,
            query_count: acc.query_count,
            batch_insert_count: acc.batch_insert_count,
            avg_insert_latency_us,
            avg_update_latency_us,
            avg_delete_latency_us,
            avg_query_latency_us,
            insert_errors: acc.insert_errors,
            update_errors: acc.update_errors,
            delete_errors: acc.delete_errors,
            query_errors: acc.query_errors,
            hot_tier_documents,
            cold_tier_documents,
            hot_tier_bytes,
            cold_tier_bytes,
            fulltext_index_size_bytes,
            path_indexes_count,
            array_indexes_count,
            collections_created: acc.collections_created,
            collections_deleted: acc.collections_deleted,
        })
    }

    /// Estimate collection statistics (placeholder for MVP)
    async fn estimate_collection_stats(&self) -> (u64, u64, u64) {
        // For MVP: Return estimated values
        // In production, this would query DocumentService.list_collections()
        (
            5,                // total_collections
            1000,             // total_documents
            10 * 1024 * 1024, // 10MB total storage
        )
    }

    /// Estimate tiering statistics (placeholder for MVP)
    async fn estimate_tiering_stats(&self) -> (u64, u64, u64, u64) {
        // For MVP: Return estimated values based on typical hot/cold distribution
        // In production, this would query the actual document store tiers
        (
            800,             // hot_tier_documents (80% in hot tier)
            200,             // cold_tier_documents (20% in cold tier)
            8 * 1024 * 1024, // hot_tier_bytes
            2 * 1024 * 1024, // cold_tier_bytes
        )
    }

    /// Estimate index statistics (placeholder for MVP)
    async fn estimate_index_stats(&self) -> (u64, u64, u64) {
        // For MVP: Return estimated values
        // In production, this would query IndexManager for actual stats
        (
            5 * 1024 * 1024, // fulltext_index_size_bytes (5MB)
            10,              // path_indexes_count
            5,               // array_indexes_count
        )
    }

    /// Calculate derived metrics from current and previous samples
    fn calculate_derived_metrics(
        &self,
        current: &DocumentMetricsSample,
        previous: Option<&DocumentMetricsSample>,
    ) -> HashMap<String, f64> {
        let mut metrics = HashMap::new();

        // Collection metrics (with proximadb_ prefix)
        metrics.insert(
            "proximadb_document_collections_total".to_string(),
            current.total_collections as f64,
        );
        metrics.insert(
            "proximadb_document_documents_total".to_string(),
            current.total_documents as f64,
        );
        metrics.insert(
            "proximadb_document_storage_bytes".to_string(),
            current.total_storage_bytes as f64,
        );

        // Operation counts
        metrics.insert(
            "proximadb_document_insert_total".to_string(),
            current.insert_count as f64,
        );
        metrics.insert(
            "proximadb_document_update_total".to_string(),
            current.update_count as f64,
        );
        metrics.insert(
            "proximadb_document_delete_total".to_string(),
            current.delete_count as f64,
        );
        metrics.insert(
            "proximadb_document_query_total".to_string(),
            current.query_count as f64,
        );
        metrics.insert(
            "proximadb_document_batch_insert_total".to_string(),
            current.batch_insert_count as f64,
        );

        // Operation latencies (in microseconds)
        metrics.insert(
            "proximadb_document_insert_latency_avg_us".to_string(),
            current.avg_insert_latency_us,
        );
        metrics.insert(
            "proximadb_document_update_latency_avg_us".to_string(),
            current.avg_update_latency_us,
        );
        metrics.insert(
            "proximadb_document_delete_latency_avg_us".to_string(),
            current.avg_delete_latency_us,
        );
        metrics.insert(
            "proximadb_document_query_latency_avg_us".to_string(),
            current.avg_query_latency_us,
        );

        // Error counts
        metrics.insert(
            "proximadb_document_insert_errors_total".to_string(),
            current.insert_errors as f64,
        );
        metrics.insert(
            "proximadb_document_update_errors_total".to_string(),
            current.update_errors as f64,
        );
        metrics.insert(
            "proximadb_document_delete_errors_total".to_string(),
            current.delete_errors as f64,
        );
        metrics.insert(
            "proximadb_document_query_errors_total".to_string(),
            current.query_errors as f64,
        );

        // Success rates
        let total_insert_ops = current.insert_count;
        if total_insert_ops > 0 {
            let success_rate = ((total_insert_ops - current.insert_errors) as f64
                / total_insert_ops as f64)
                * 100.0;
            metrics.insert(
                "proximadb_document_insert_success_rate_percent".to_string(),
                success_rate,
            );
        }

        let total_query_ops = current.query_count;
        if total_query_ops > 0 {
            let success_rate =
                ((total_query_ops - current.query_errors) as f64 / total_query_ops as f64) * 100.0;
            metrics.insert(
                "proximadb_document_query_success_rate_percent".to_string(),
                success_rate,
            );
        }

        // Tiering metrics
        metrics.insert(
            "proximadb_document_hot_tier_documents".to_string(),
            current.hot_tier_documents as f64,
        );
        metrics.insert(
            "proximadb_document_cold_tier_documents".to_string(),
            current.cold_tier_documents as f64,
        );
        metrics.insert(
            "proximadb_document_hot_tier_bytes".to_string(),
            current.hot_tier_bytes as f64,
        );
        metrics.insert(
            "proximadb_document_cold_tier_bytes".to_string(),
            current.cold_tier_bytes as f64,
        );

        // Hot tier ratio
        let total_tiered_docs = current.hot_tier_documents + current.cold_tier_documents;
        if total_tiered_docs > 0 {
            let hot_ratio = current.hot_tier_documents as f64 / total_tiered_docs as f64;
            metrics.insert("proximadb_document_hot_tier_ratio".to_string(), hot_ratio);
        }

        // Index metrics
        metrics.insert(
            "proximadb_document_fulltext_index_bytes".to_string(),
            current.fulltext_index_size_bytes as f64,
        );
        metrics.insert(
            "proximadb_document_path_indexes_count".to_string(),
            current.path_indexes_count as f64,
        );
        metrics.insert(
            "proximadb_document_array_indexes_count".to_string(),
            current.array_indexes_count as f64,
        );

        // Collection lifecycle
        metrics.insert(
            "proximadb_document_collections_created_total".to_string(),
            current.collections_created as f64,
        );
        metrics.insert(
            "proximadb_document_collections_deleted_total".to_string(),
            current.collections_deleted as f64,
        );

        // Calculate rates if we have previous sample
        if let Some(prev) = previous {
            let time_diff = current
                .timestamp
                .duration_since(prev.timestamp)
                .as_secs_f64();
            if time_diff > 0.0 {
                // Insert rate
                let insert_rate = (current.insert_count - prev.insert_count) as f64 / time_diff;
                metrics.insert(
                    "proximadb_document_inserts_per_second".to_string(),
                    insert_rate,
                );

                // Query rate
                let query_rate = (current.query_count - prev.query_count) as f64 / time_diff;
                metrics.insert(
                    "proximadb_document_queries_per_second".to_string(),
                    query_rate,
                );

                // Update rate
                let update_rate = (current.update_count - prev.update_count) as f64 / time_diff;
                metrics.insert(
                    "proximadb_document_updates_per_second".to_string(),
                    update_rate,
                );

                // Delete rate
                let delete_rate = (current.delete_count - prev.delete_count) as f64 / time_diff;
                metrics.insert(
                    "proximadb_document_deletes_per_second".to_string(),
                    delete_rate,
                );

                // Total operations rate
                let total_ops_current = current.insert_count
                    + current.update_count
                    + current.delete_count
                    + current.query_count;
                let total_ops_prev =
                    prev.insert_count + prev.update_count + prev.delete_count + prev.query_count;
                let ops_rate = (total_ops_current - total_ops_prev) as f64 / time_diff;
                metrics.insert(
                    "proximadb_document_operations_per_second".to_string(),
                    ops_rate,
                );
            }
        }

        metrics
    }
}

impl Default for DocumentMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetricsCollector for DocumentMetricsCollector {
    async fn collect(&self) -> Result<MetricsSample> {
        // Collect current document metrics
        let current_sample = self.collect_document_metrics().await?;

        // Get previous sample for rate calculations
        let mut last_sample_guard = self.last_sample.write().await;
        let previous_sample = last_sample_guard.as_ref();

        // Calculate all metrics including derived ones
        let values = self.calculate_derived_metrics(&current_sample, previous_sample);

        // Store current sample for next collection
        *last_sample_guard = Some(current_sample.clone());
        drop(last_sample_guard);

        Ok(MetricsSample {
            timestamp: current_sample.timestamp,
            collector: self.name.to_string(),
            values,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn recommended_interval(&self) -> Duration {
        Duration::from_secs(60) // Collect document metrics every minute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_document_metrics_collector_new() {
        let collector = DocumentMetricsCollector::new();
        assert_eq!(collector.name(), "document_store");
        assert_eq!(collector.recommended_interval(), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_document_metrics_collector_collect() {
        let collector = DocumentMetricsCollector::new();

        let sample = collector.collect().await.unwrap();
        assert_eq!(sample.collector, "document_store");
        assert!(
            sample
                .values
                .contains_key("proximadb_document_collections_total")
        );
        assert!(
            sample
                .values
                .contains_key("proximadb_document_documents_total")
        );
        assert!(
            sample
                .values
                .contains_key("proximadb_document_insert_total")
        );
        assert!(sample.values.contains_key("proximadb_document_query_total"));
    }

    #[tokio::test]
    async fn test_document_metrics_record_operations() {
        let collector = DocumentMetricsCollector::new();

        // Record some operations
        collector.record_insert(100.0, false).await;
        collector.record_insert(150.0, false).await;
        collector.record_insert(200.0, true).await;
        collector.record_update(50.0, false).await;
        collector.record_delete(30.0, false).await;
        collector.record_query(500.0, false).await;
        collector.record_query(600.0, true).await;

        let sample = collector.collect().await.unwrap();

        // Check insert metrics
        assert_eq!(
            sample.values.get("proximadb_document_insert_total"),
            Some(&3.0)
        );
        assert_eq!(
            sample.values.get("proximadb_document_insert_errors_total"),
            Some(&1.0)
        );

        // Check update metrics
        assert_eq!(
            sample.values.get("proximadb_document_update_total"),
            Some(&1.0)
        );

        // Check delete metrics
        assert_eq!(
            sample.values.get("proximadb_document_delete_total"),
            Some(&1.0)
        );

        // Check query metrics
        assert_eq!(
            sample.values.get("proximadb_document_query_total"),
            Some(&2.0)
        );
        assert_eq!(
            sample.values.get("proximadb_document_query_errors_total"),
            Some(&1.0)
        );
    }

    #[tokio::test]
    async fn test_document_metrics_batch_insert() {
        let collector = DocumentMetricsCollector::new();

        // Record a batch insert of 100 documents
        collector.record_batch_insert(100, 5000.0, false).await;

        let sample = collector.collect().await.unwrap();

        assert_eq!(
            sample.values.get("proximadb_document_batch_insert_total"),
            Some(&1.0)
        );
        assert_eq!(
            sample.values.get("proximadb_document_insert_total"),
            Some(&100.0)
        );
    }

    #[tokio::test]
    async fn test_document_metrics_collection_lifecycle() {
        let collector = DocumentMetricsCollector::new();

        // Record collection events
        collector.record_collection_created().await;
        collector.record_collection_created().await;
        collector.record_collection_deleted().await;

        let sample = collector.collect().await.unwrap();

        assert_eq!(
            sample
                .values
                .get("proximadb_document_collections_created_total"),
            Some(&2.0)
        );
        assert_eq!(
            sample
                .values
                .get("proximadb_document_collections_deleted_total"),
            Some(&1.0)
        );
    }

    #[tokio::test]
    async fn test_document_metrics_tiering() {
        let collector = DocumentMetricsCollector::new();

        let sample = collector.collect().await.unwrap();

        // Check tiering metrics are present
        assert!(
            sample
                .values
                .contains_key("proximadb_document_hot_tier_documents")
        );
        assert!(
            sample
                .values
                .contains_key("proximadb_document_cold_tier_documents")
        );
        assert!(
            sample
                .values
                .contains_key("proximadb_document_hot_tier_ratio")
        );
    }

    #[tokio::test]
    async fn test_document_metrics_index_stats() {
        let collector = DocumentMetricsCollector::new();

        let sample = collector.collect().await.unwrap();

        // Check index metrics are present
        assert!(
            sample
                .values
                .contains_key("proximadb_document_fulltext_index_bytes")
        );
        assert!(
            sample
                .values
                .contains_key("proximadb_document_path_indexes_count")
        );
        assert!(
            sample
                .values
                .contains_key("proximadb_document_array_indexes_count")
        );
    }

    #[tokio::test]
    async fn test_document_metrics_success_rates() {
        let collector = DocumentMetricsCollector::new();

        // Record some operations with errors
        for _ in 0..90 {
            collector.record_insert(100.0, false).await;
        }
        for _ in 0..10 {
            collector.record_insert(100.0, true).await;
        }

        let sample = collector.collect().await.unwrap();

        // 90 successful out of 100 = 90% success rate
        if let Some(&success_rate) = sample
            .values
            .get("proximadb_document_insert_success_rate_percent")
        {
            assert!((success_rate - 90.0).abs() < 0.01);
        }
    }

    #[tokio::test]
    async fn test_document_metrics_rate_calculation() {
        let collector = DocumentMetricsCollector::new();

        // First collection
        collector.record_insert(100.0, false).await;
        let _ = collector.collect().await.unwrap();

        // Wait a bit and record more operations
        tokio::time::sleep(Duration::from_millis(100)).await;

        collector.record_insert(100.0, false).await;
        collector.record_insert(100.0, false).await;
        let sample = collector.collect().await.unwrap();

        // Rate calculations should be present (based on time between collections)
        assert!(
            sample
                .values
                .contains_key("proximadb_document_inserts_per_second")
        );
    }

    #[tokio::test]
    async fn test_document_metrics_default() {
        let collector = DocumentMetricsCollector::default();
        assert_eq!(collector.name(), "document_store");
    }
}
