//! Prometheus format metrics exporter
//!
//! Exports ProximaDB metrics in Prometheus text format (version 0.0.4).
//! All metrics are prefixed with `proximadb_` to avoid naming collisions.

use super::{MetricsExportSnapshot, MetricsExporter};
use anyhow::Result;
use std::fmt::Write;

/// Prometheus exporter for ProximaDB metrics.
///
/// Exports metrics in Prometheus exposition format with `proximadb_` prefix.
pub struct PrometheusExporter;

impl PrometheusExporter {
    pub fn new() -> Self {
        Self
    }

    /// Export system metrics only (for backward compatibility)
    pub fn export_system_metrics(&self, metrics: &super::SystemMetrics) -> Result<String> {
        let mut output = String::new();

        // System metrics - CPU
        writeln!(
            output,
            "# HELP proximadb_cpu_usage_percent Current CPU usage percentage"
        )?;
        writeln!(output, "# TYPE proximadb_cpu_usage_percent gauge")?;
        writeln!(output, "proximadb_cpu_usage_percent {}", metrics.cpu_usage)?;

        // System metrics - Memory
        writeln!(
            output,
            "# HELP proximadb_memory_used_bytes Current memory used in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_memory_used_bytes gauge")?;
        writeln!(
            output,
            "proximadb_memory_used_bytes {}",
            metrics.memory_used_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_memory_total_bytes Total system memory in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_memory_total_bytes gauge")?;
        writeln!(
            output,
            "proximadb_memory_total_bytes {}",
            metrics.memory_total_bytes
        )?;

        // System metrics - Disk
        writeln!(
            output,
            "# HELP proximadb_disk_used_bytes Disk space used in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_disk_used_bytes gauge")?;
        writeln!(
            output,
            "proximadb_disk_used_bytes {}",
            metrics.disk_used_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_disk_total_bytes Total disk space in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_disk_total_bytes gauge")?;
        writeln!(
            output,
            "proximadb_disk_total_bytes {}",
            metrics.disk_total_bytes
        )?;

        // Server metrics
        writeln!(
            output,
            "# HELP proximadb_uptime_seconds Server uptime in seconds"
        )?;
        writeln!(output, "# TYPE proximadb_uptime_seconds counter")?;
        writeln!(
            output,
            "proximadb_uptime_seconds {}",
            metrics.server.uptime_seconds
        )?;

        // Storage metrics
        writeln!(
            output,
            "# HELP proximadb_vectors_total Total number of vectors stored"
        )?;
        writeln!(output, "# TYPE proximadb_vectors_total gauge")?;
        writeln!(
            output,
            "proximadb_vectors_total {}",
            metrics.storage.total_vectors
        )?;

        writeln!(
            output,
            "# HELP proximadb_collections_total Total number of collections"
        )?;
        writeln!(output, "# TYPE proximadb_collections_total gauge")?;
        writeln!(
            output,
            "proximadb_collections_total {}",
            metrics.storage.total_collections
        )?;

        writeln!(
            output,
            "# HELP proximadb_storage_bytes Total storage size in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_storage_bytes gauge")?;
        writeln!(
            output,
            "proximadb_storage_bytes {}",
            metrics.storage.storage_size_bytes
        )?;

        // Query metrics
        writeln!(
            output,
            "# HELP proximadb_queries_total Total number of queries processed"
        )?;
        writeln!(output, "# TYPE proximadb_queries_total counter")?;
        writeln!(
            output,
            "proximadb_queries_total {}",
            metrics.query.total_queries
        )?;

        writeln!(
            output,
            "# HELP proximadb_queries_failed_total Total number of failed queries"
        )?;
        writeln!(output, "# TYPE proximadb_queries_failed_total counter")?;
        writeln!(
            output,
            "proximadb_queries_failed_total {}",
            metrics.query.failed_queries
        )?;

        writeln!(
            output,
            "# HELP proximadb_query_latency_p99_ms 99th percentile query latency in milliseconds"
        )?;
        writeln!(output, "# TYPE proximadb_query_latency_p99_ms gauge")?;
        writeln!(
            output,
            "proximadb_query_latency_p99_ms {}",
            metrics.query.p99_latency_ms
        )?;

        // Index metrics
        writeln!(
            output,
            "# HELP proximadb_indexes_total Total number of indexes"
        )?;
        writeln!(output, "# TYPE proximadb_indexes_total gauge")?;
        writeln!(
            output,
            "proximadb_indexes_total {}",
            metrics.index.total_indexes
        )?;

        writeln!(
            output,
            "# HELP proximadb_index_memory_bytes Memory used by indexes in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_index_memory_bytes gauge")?;
        writeln!(
            output,
            "proximadb_index_memory_bytes {}",
            metrics.index.index_memory_usage_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_search_operations_per_second Current search operations per second"
        )?;
        writeln!(
            output,
            "# TYPE proximadb_search_operations_per_second gauge"
        )?;
        writeln!(
            output,
            "proximadb_search_operations_per_second {}",
            metrics.index.search_operations_per_second
        )?;

        Ok(output)
    }
}

impl Default for PrometheusExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsExporter for PrometheusExporter {
    fn export(&self, metrics: &MetricsExportSnapshot) -> Result<String> {
        let mut output = String::new();

        // System metrics - CPU
        writeln!(
            output,
            "# HELP proximadb_cpu_usage_percent Current CPU usage percentage"
        )?;
        writeln!(output, "# TYPE proximadb_cpu_usage_percent gauge")?;
        writeln!(
            output,
            "proximadb_cpu_usage_percent {}",
            metrics.system.cpu_usage
        )?;

        // System metrics - Memory
        writeln!(
            output,
            "# HELP proximadb_memory_used_bytes Current memory used in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_memory_used_bytes gauge")?;
        writeln!(
            output,
            "proximadb_memory_used_bytes {}",
            metrics.system.memory_used_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_memory_total_bytes Total system memory in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_memory_total_bytes gauge")?;
        writeln!(
            output,
            "proximadb_memory_total_bytes {}",
            metrics.system.memory_total_bytes
        )?;

        // System metrics - Disk
        writeln!(
            output,
            "# HELP proximadb_disk_used_bytes Disk space used in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_disk_used_bytes gauge")?;
        writeln!(
            output,
            "proximadb_disk_used_bytes {}",
            metrics.system.disk_used_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_disk_total_bytes Total disk space in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_disk_total_bytes gauge")?;
        writeln!(
            output,
            "proximadb_disk_total_bytes {}",
            metrics.system.disk_total_bytes
        )?;

        // Server metrics
        writeln!(
            output,
            "# HELP proximadb_uptime_seconds Server uptime in seconds"
        )?;
        writeln!(output, "# TYPE proximadb_uptime_seconds counter")?;
        writeln!(
            output,
            "proximadb_uptime_seconds {}",
            metrics.system.uptime_seconds
        )?;

        // Collection metrics - with proper Prometheus format (labels only on metric line)
        // Note: HELP and TYPE declarations should NOT include labels
        if !metrics.collections.is_empty() {
            writeln!(
                output,
                "# HELP proximadb_collection_vectors_total Total vectors in collection"
            )?;
            writeln!(output, "# TYPE proximadb_collection_vectors_total gauge")?;
            for (collection_id, col_metrics) in &metrics.collections {
                writeln!(
                    output,
                    "proximadb_collection_vectors_total{{collection=\"{}\"}} {}",
                    collection_id, col_metrics.vector_count
                )?;
            }

            writeln!(
                output,
                "# HELP proximadb_collection_search_qps Search queries per second by collection"
            )?;
            writeln!(output, "# TYPE proximadb_collection_search_qps gauge")?;
            for (collection_id, col_metrics) in &metrics.collections {
                writeln!(
                    output,
                    "proximadb_collection_search_qps{{collection=\"{}\"}} {}",
                    collection_id, col_metrics.search_qps
                )?;
            }

            writeln!(
                output,
                "# HELP proximadb_collection_insert_qps Insert queries per second by collection"
            )?;
            writeln!(output, "# TYPE proximadb_collection_insert_qps gauge")?;
            for (collection_id, col_metrics) in &metrics.collections {
                writeln!(
                    output,
                    "proximadb_collection_insert_qps{{collection=\"{}\"}} {}",
                    collection_id, col_metrics.insert_qps
                )?;
            }

            writeln!(
                output,
                "# HELP proximadb_collection_latency_p99_ms 99th percentile latency by collection"
            )?;
            writeln!(output, "# TYPE proximadb_collection_latency_p99_ms gauge")?;
            for (collection_id, col_metrics) in &metrics.collections {
                writeln!(
                    output,
                    "proximadb_collection_latency_p99_ms{{collection=\"{}\"}} {}",
                    collection_id, col_metrics.p99_latency_ms
                )?;
            }

            writeln!(
                output,
                "# HELP proximadb_collection_cache_hit_rate Cache hit rate by collection"
            )?;
            writeln!(output, "# TYPE proximadb_collection_cache_hit_rate gauge")?;
            for (collection_id, col_metrics) in &metrics.collections {
                writeln!(
                    output,
                    "proximadb_collection_cache_hit_rate{{collection=\"{}\"}} {}",
                    collection_id, col_metrics.cache_hit_rate
                )?;
            }

            writeln!(
                output,
                "# HELP proximadb_collection_index_size_bytes Index size in bytes by collection"
            )?;
            writeln!(output, "# TYPE proximadb_collection_index_size_bytes gauge")?;
            for (collection_id, col_metrics) in &metrics.collections {
                writeln!(
                    output,
                    "proximadb_collection_index_size_bytes{{collection=\"{}\"}} {}",
                    collection_id, col_metrics.index_size_bytes
                )?;
            }
        }

        // Cache metrics
        writeln!(
            output,
            "# HELP proximadb_cache_hit_rate Overall cache hit rate"
        )?;
        writeln!(output, "# TYPE proximadb_cache_hit_rate gauge")?;
        writeln!(
            output,
            "proximadb_cache_hit_rate {}",
            metrics.cache.hit_rate
        )?;

        writeln!(
            output,
            "# HELP proximadb_cache_evictions_per_second Cache evictions per second"
        )?;
        writeln!(output, "# TYPE proximadb_cache_evictions_per_second gauge")?;
        writeln!(
            output,
            "proximadb_cache_evictions_per_second {}",
            metrics.cache.evictions_per_second
        )?;

        writeln!(
            output,
            "# HELP proximadb_cache_memory_bytes Memory used by cache in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_cache_memory_bytes gauge")?;
        writeln!(
            output,
            "proximadb_cache_memory_bytes {}",
            metrics.cache.memory_used_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_cache_entries_total Total number of cache entries"
        )?;
        writeln!(output, "# TYPE proximadb_cache_entries_total gauge")?;
        writeln!(
            output,
            "proximadb_cache_entries_total {}",
            metrics.cache.entries_count
        )?;

        // Compression metrics
        writeln!(
            output,
            "# HELP proximadb_compression_ratio Current compression ratio"
        )?;
        writeln!(output, "# TYPE proximadb_compression_ratio gauge")?;
        writeln!(
            output,
            "proximadb_compression_ratio {}",
            metrics.compression.compression_ratio
        )?;

        writeln!(
            output,
            "# HELP proximadb_compressed_bytes Total compressed data in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_compressed_bytes gauge")?;
        writeln!(
            output,
            "proximadb_compressed_bytes {}",
            metrics.compression.compressed_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_uncompressed_bytes Total uncompressed data in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_uncompressed_bytes gauge")?;
        writeln!(
            output,
            "proximadb_uncompressed_bytes {}",
            metrics.compression.uncompressed_bytes
        )?;

        // T2.2: Cache hit-rate metrics
        writeln!(
            output,
            "# HELP proximadb_cache_hit_rate Overall cache hit rate (0-1)"
        )?;
        writeln!(output, "# TYPE proximadb_cache_hit_rate gauge")?;
        writeln!(
            output,
            "proximadb_cache_hit_rate {}",
            metrics.cache.hit_rate
        )?;

        writeln!(
            output,
            "# HELP proximadb_cache_evictions_per_second Cache evictions per second"
        )?;
        writeln!(output, "# TYPE proximadb_cache_evictions_per_second gauge")?;
        writeln!(
            output,
            "proximadb_cache_evictions_per_second {}",
            metrics.cache.evictions_per_second
        )?;

        writeln!(
            output,
            "# HELP proximadb_cache_memory_used_bytes Cache memory used in bytes"
        )?;
        writeln!(output, "# TYPE proximadb_cache_memory_used_bytes gauge")?;
        writeln!(
            output,
            "proximadb_cache_memory_used_bytes {}",
            metrics.cache.memory_used_bytes
        )?;

        writeln!(
            output,
            "# HELP proximadb_cache_entries_count Total cache entries"
        )?;
        writeln!(output, "# TYPE proximadb_cache_entries_count gauge")?;
        writeln!(
            output,
            "proximadb_cache_entries_count {}",
            metrics.cache.entries_count
        )?;

        // T1.1: Fusion metrics
        writeln!(
            output,
            "# HELP proximadb_fusion_total Total number of fusion operations"
        )?;
        writeln!(output, "# TYPE proximadb_fusion_total counter")?;
        writeln!(
            output,
            "proximadb_fusion_total {}",
            metrics.fusion.total_fusions
        )?;

        writeln!(
            output,
            "# HELP proximadb_fusion_sources_fused_total Total sources fused across all operations"
        )?;
        writeln!(
            output,
            "# TYPE proximadb_fusion_sources_fused_total counter"
        )?;
        writeln!(
            output,
            "proximadb_fusion_sources_fused_total {}",
            metrics.fusion.total_sources_fused
        )?;

        writeln!(
            output,
            "# HELP proximadb_fusion_sources_skipped_total Total sources skipped across all operations"
        )?;
        writeln!(
            output,
            "# TYPE proximadb_fusion_sources_skipped_total counter"
        )?;
        writeln!(
            output,
            "proximadb_fusion_sources_skipped_total {}",
            metrics.fusion.total_sources_skipped
        )?;

        writeln!(
            output,
            "# HELP proximadb_fusion_latency_seconds_avg Average fusion latency in seconds"
        )?;
        writeln!(output, "# TYPE proximadb_fusion_latency_seconds_avg gauge")?;
        writeln!(
            output,
            "proximadb_fusion_latency_seconds_avg {}",
            metrics.fusion.avg_latency_seconds
        )?;

        Ok(output)
    }

    fn content_type(&self) -> &'static str {
        "text/plain; version=0.0.4; charset=utf-8"
    }

    fn format_name(&self) -> &'static str {
        "prometheus"
    }
}
