// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics exporters for different formats (Prometheus, JSON, etc.)

use std::collections::HashMap;
use std::fmt::Write;
use anyhow::Result;
use serde_json;

use super::{Metric, MetricType, MetricValue, SystemMetrics, Histogram, Summary};

/// Prometheus format exporter
pub struct PrometheusExporter;

/// JSON format exporter
pub struct JsonExporter;

/// OpenTelemetry format exporter
pub struct OpenTelemetryExporter;

/// Trait for metric exporters
pub trait MetricsExporter {
    fn export_metrics(&self, metrics: &[Metric]) -> Result<String>;
    fn export_system_metrics(&self, system_metrics: &SystemMetrics) -> Result<String>;
    fn content_type(&self) -> &'static str;
}

impl PrometheusExporter {
    pub fn new() -> Self {
        Self
    }
    
    /// Format a single metric value for Prometheus
    fn format_metric_value(&self, metric: &Metric, value: &MetricValue) -> String {
        let mut result = String::new();
        
        // Add labels if present
        let labels = if value.labels.is_empty() {
            String::new()
        } else {
            let label_str: Vec<String> = value.labels.iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            format!("{{{}}}", label_str.join(","))
        };
        
        match metric.metric_type {
            MetricType::Counter | MetricType::Gauge => {
                writeln!(result, "{}{} {}", metric.name, labels, value.value).unwrap();
            }
            MetricType::Histogram => {
                // Histogram metrics require special formatting
                writeln!(result, "# HELP {} {}", metric.name, metric.help).unwrap();
                writeln!(result, "# TYPE {} histogram", metric.name).unwrap();
                writeln!(result, "{}_count{} {}", metric.name, labels, value.value).unwrap();
            }
            MetricType::Summary => {
                // Summary metrics require special formatting
                writeln!(result, "# HELP {} {}", metric.name, metric.help).unwrap();
                writeln!(result, "# TYPE {} summary", metric.name).unwrap();
                writeln!(result, "{}_count{} {}", metric.name, labels, value.value).unwrap();
            }
        }
        
        result
    }
    
    /// Format histogram data for Prometheus
    fn format_histogram(&self, name: &str, histogram: &Histogram, labels: &HashMap<String, String>) -> String {
        let mut result = String::new();
        
        let label_str = if labels.is_empty() {
            String::new()
        } else {
            let label_parts: Vec<String> = labels.iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            format!("{{{}}}", label_parts.join(","))
        };
        
        // Bucket counts
        for bucket in &histogram.buckets {
            let bucket_labels = if labels.is_empty() {
                format!("{{le=\"{}\"}}", bucket.upper_bound)
            } else {
                let mut all_labels = labels.clone();
                all_labels.insert("le".to_string(), bucket.upper_bound.to_string());
                let label_parts: Vec<String> = all_labels.iter()
                    .map(|(k, v)| format!("{}=\"{}\"", k, v))
                    .collect();
                format!("{{{}}}", label_parts.join(","))
            };
            
            writeln!(result, "{}_bucket{} {}", name, bucket_labels, bucket.count).unwrap();
        }
        
        // Add +Inf bucket
        let inf_labels = if labels.is_empty() {
            "{le=\"+Inf\"}".to_string()
        } else {
            let mut all_labels = labels.clone();
            all_labels.insert("le".to_string(), "+Inf".to_string());
            let label_parts: Vec<String> = all_labels.iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            format!("{{{}}}", label_parts.join(","))
        };
        writeln!(result, "{}_bucket{} {}", name, inf_labels, histogram.count).unwrap();
        
        // Sum and count
        writeln!(result, "{}_sum{} {}", name, label_str, histogram.sum).unwrap();
        writeln!(result, "{}_count{} {}", name, label_str, histogram.count).unwrap();
        
        result
    }
    
    /// Format summary data for Prometheus
    fn format_summary(&self, name: &str, summary: &Summary, labels: &HashMap<String, String>) -> String {
        let mut result = String::new();
        
        let label_str = if labels.is_empty() {
            String::new()
        } else {
            let label_parts: Vec<String> = labels.iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            format!("{{{}}}", label_parts.join(","))
        };
        
        // Quantiles
        for (quantile, value) in &summary.quantiles {
            let quantile_labels = if labels.is_empty() {
                format!("{{quantile=\"{}\"}}", quantile)
            } else {
                let mut all_labels = labels.clone();
                all_labels.insert("quantile".to_string(), quantile.to_string());
                let label_parts: Vec<String> = all_labels.iter()
                    .map(|(k, v)| format!("{}=\"{}\"", k, v))
                    .collect();
                format!("{{{}}}", label_parts.join(","))
            };
            
            writeln!(result, "{}{} {}", name, quantile_labels, value).unwrap();
        }
        
        // Sum and count
        writeln!(result, "{}_sum{} {}", name, label_str, summary.sum).unwrap();
        writeln!(result, "{}_count{} {}", name, label_str, summary.count).unwrap();
        
        result
    }
}

impl MetricsExporter for PrometheusExporter {
    fn export_metrics(&self, metrics: &[Metric]) -> Result<String> {
        let mut output = String::new();
        
        for metric in metrics {
            // Add help and type comments
            writeln!(output, "# HELP {} {}", metric.name, metric.help)?;
            writeln!(output, "# TYPE {} {}", metric.name, 
                match metric.metric_type {
                    MetricType::Counter => "counter",
                    MetricType::Gauge => "gauge", 
                    MetricType::Histogram => "histogram",
                    MetricType::Summary => "summary",
                })?;
            
            // Export metric values
            for value in &metric.values {
                output.push_str(&self.format_metric_value(metric, value));
            }
            
            output.push('\n');
        }
        
        Ok(output)
    }
    
    fn export_system_metrics(&self, system_metrics: &SystemMetrics) -> Result<String> {
        let mut output = String::new();
        
        // Server metrics
        writeln!(output, "# HELP proximadb_uptime_seconds Server uptime in seconds")?;
        writeln!(output, "# TYPE proximadb_uptime_seconds counter")?;
        writeln!(output, "proximadb_uptime_seconds {}", system_metrics.server.uptime_seconds)?;
        
        writeln!(output, "# HELP proximadb_cpu_usage_percent CPU usage percentage")?;
        writeln!(output, "# TYPE proximadb_cpu_usage_percent gauge")?;
        writeln!(output, "proximadb_cpu_usage_percent {}", system_metrics.server.cpu_usage_percent)?;
        
        writeln!(output, "# HELP proximadb_memory_usage_bytes Memory usage in bytes")?;
        writeln!(output, "# TYPE proximadb_memory_usage_bytes gauge")?;
        writeln!(output, "proximadb_memory_usage_bytes {}", system_metrics.server.memory_usage_bytes)?;
        
        writeln!(output, "# HELP proximadb_memory_available_bytes Available memory in bytes")?;
        writeln!(output, "# TYPE proximadb_memory_available_bytes gauge")?;
        writeln!(output, "proximadb_memory_available_bytes {}", system_metrics.server.memory_available_bytes)?;
        
        // Storage metrics
        writeln!(output, "# HELP proximadb_total_vectors Total number of vectors stored")?;
        writeln!(output, "# TYPE proximadb_total_vectors gauge")?;
        writeln!(output, "proximadb_total_vectors {}", system_metrics.storage.total_vectors)?;
        
        writeln!(output, "# HELP proximadb_total_collections Total number of collections")?;
        writeln!(output, "# TYPE proximadb_total_collections gauge")?;
        writeln!(output, "proximadb_total_collections {}", system_metrics.storage.total_collections)?;
        
        writeln!(output, "# HELP proximadb_storage_size_bytes Total storage size in bytes")?;
        writeln!(output, "# TYPE proximadb_storage_size_bytes gauge")?;
        writeln!(output, "proximadb_storage_size_bytes {}", system_metrics.storage.storage_size_bytes)?;
        
        writeln!(output, "# HELP proximadb_cache_hit_rate Cache hit rate (0.0 to 1.0)")?;
        writeln!(output, "# TYPE proximadb_cache_hit_rate gauge")?;
        writeln!(output, "proximadb_cache_hit_rate {}", system_metrics.storage.cache_hit_rate)?;
        
        // Query metrics
        writeln!(output, "# HELP proximadb_total_queries Total number of queries processed")?;
        writeln!(output, "# TYPE proximadb_total_queries counter")?;
        writeln!(output, "proximadb_total_queries {}", system_metrics.query.total_queries)?;
        
        writeln!(output, "# HELP proximadb_queries_per_second Current queries per second")?;
        writeln!(output, "# TYPE proximadb_queries_per_second gauge")?;
        writeln!(output, "proximadb_queries_per_second {}", system_metrics.query.queries_per_second)?;
        
        writeln!(output, "# HELP proximadb_query_latency_ms Query latency in milliseconds")?;
        writeln!(output, "# TYPE proximadb_query_latency_ms gauge")?;
        writeln!(output, "proximadb_query_latency_ms{{quantile=\"0.5\"}} {}", system_metrics.query.p50_query_latency_ms)?;
        writeln!(output, "proximadb_query_latency_ms{{quantile=\"0.9\"}} {}", system_metrics.query.p90_query_latency_ms)?;
        writeln!(output, "proximadb_query_latency_ms{{quantile=\"0.99\"}} {}", system_metrics.query.p99_query_latency_ms)?;
        
        // Index metrics
        writeln!(output, "# HELP proximadb_total_indexes Total number of indexes")?;
        writeln!(output, "# TYPE proximadb_total_indexes gauge")?;
        writeln!(output, "proximadb_total_indexes {}", system_metrics.index.total_indexes)?;
        
        writeln!(output, "# HELP proximadb_index_memory_usage_bytes Index memory usage in bytes")?;
        writeln!(output, "# TYPE proximadb_index_memory_usage_bytes gauge")?;
        writeln!(output, "proximadb_index_memory_usage_bytes {}", system_metrics.index.index_memory_usage_bytes)?;
        
        // Custom metrics
        for (name, value) in &system_metrics.custom {
            writeln!(output, "# HELP proximadb_custom_{} Custom metric", name)?;
            writeln!(output, "# TYPE proximadb_custom_{} gauge", name)?;
            writeln!(output, "proximadb_custom_{} {}", name, value)?;
        }
        
        Ok(output)
    }
    
    fn content_type(&self) -> &'static str {
        "text/plain; version=0.0.4"
    }
}

impl MetricsExporter for JsonExporter {
    fn export_metrics(&self, metrics: &[Metric]) -> Result<String> {
        let json_metrics = serde_json::to_string_pretty(metrics)?;
        Ok(json_metrics)
    }
    
    fn export_system_metrics(&self, system_metrics: &SystemMetrics) -> Result<String> {
        let json_metrics = serde_json::to_string_pretty(system_metrics)?;
        Ok(json_metrics)
    }
    
    fn content_type(&self) -> &'static str {
        "application/json"
    }
}

impl MetricsExporter for OpenTelemetryExporter {
    fn export_metrics(&self, metrics: &[Metric]) -> Result<String> {
        // OpenTelemetry OTLP format (simplified JSON representation)
        let mut otel_metrics = serde_json::Map::new();
        otel_metrics.insert("resourceMetrics".to_string(), serde_json::json!([{
            "resource": {
                "attributes": [
                    {
                        "key": "service.name",
                        "value": {
                            "stringValue": "proximadb"
                        }
                    },
                    {
                        "key": "service.version", 
                        "value": {
                            "stringValue": "0.1.0"
                        }
                    }
                ]
            },
            "scopeMetrics": [{
                "scope": {
                    "name": "proximadb-metrics",
                    "version": "0.1.0"
                },
                "metrics": metrics.iter().map(|m| {
                    serde_json::json!({
                        "name": m.name,
                        "description": m.help,
                        "unit": "",
                        "gauge": {
                            "dataPoints": m.values.iter().map(|v| {
                                serde_json::json!({
                                    "timeUnixNano": v.timestamp.timestamp_nanos_opt().unwrap_or(0),
                                    "asDouble": v.value,
                                    "attributes": v.labels.iter().map(|(k, v)| {
                                        serde_json::json!({
                                            "key": k,
                                            "value": {
                                                "stringValue": v
                                            }
                                        })
                                    }).collect::<Vec<_>>()
                                })
                            }).collect::<Vec<_>>()
                        }
                    })
                }).collect::<Vec<_>>()
            }]
        }]));
        
        Ok(serde_json::to_string_pretty(&otel_metrics)?)
    }
    
    fn export_system_metrics(&self, system_metrics: &SystemMetrics) -> Result<String> {
        // Convert system metrics to OTLP format
        let timestamp_nanos = system_metrics.timestamp.timestamp_nanos_opt().unwrap_or(0);
        
        let metrics_data = vec![
            ("proximadb.uptime.seconds", system_metrics.server.uptime_seconds),
            ("proximadb.cpu.usage.percent", system_metrics.server.cpu_usage_percent),
            ("proximadb.memory.usage.bytes", system_metrics.server.memory_usage_bytes as f64),
            ("proximadb.storage.vectors.total", system_metrics.storage.total_vectors as f64),
            ("proximadb.storage.collections.total", system_metrics.storage.total_collections as f64),
            ("proximadb.queries.total", system_metrics.query.total_queries as f64),
            ("proximadb.queries.per_second", system_metrics.query.queries_per_second),
            ("proximadb.query.latency.p99.ms", system_metrics.query.p99_query_latency_ms),
        ];
        
        let otel_metrics = serde_json::json!({
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        {
                            "key": "service.name",
                            "value": {"stringValue": "proximadb"}
                        }
                    ]
                },
                "scopeMetrics": [{
                    "scope": {
                        "name": "proximadb-system-metrics"
                    },
                    "metrics": metrics_data.iter().map(|(name, value)| {
                        serde_json::json!({
                            "name": name,
                            "gauge": {
                                "dataPoints": [{
                                    "timeUnixNano": timestamp_nanos,
                                    "asDouble": value
                                }]
                            }
                        })
                    }).collect::<Vec<_>>()
                }]
            }]
        });
        
        Ok(serde_json::to_string_pretty(&otel_metrics)?)
    }
    
    fn content_type(&self) -> &'static str {
        "application/json"
    }
}

/// Metrics formatting utilities
pub struct MetricsFormatter;

impl MetricsFormatter {
    /// Format metrics for human-readable display
    pub fn format_for_display(system_metrics: &SystemMetrics) -> String {
        let mut output = String::new();
        
        writeln!(output, "ProximaDB System Metrics - {}", system_metrics.timestamp.format("%Y-%m-%d %H:%M:%S UTC")).unwrap();
        writeln!(output, "=====================================").unwrap();
        
        writeln!(output, "\nServer Metrics:").unwrap();
        writeln!(output, "  Uptime: {:.1} seconds", system_metrics.server.uptime_seconds).unwrap();
        writeln!(output, "  CPU Usage: {:.1}%", system_metrics.server.cpu_usage_percent).unwrap();
        writeln!(output, "  Memory Usage: {:.1} MB", system_metrics.server.memory_usage_bytes as f64 / 1024.0 / 1024.0).unwrap();
        writeln!(output, "  Memory Available: {:.1} MB", system_metrics.server.memory_available_bytes as f64 / 1024.0 / 1024.0).unwrap();
        
        writeln!(output, "\nStorage Metrics:").unwrap();
        writeln!(output, "  Total Vectors: {}", system_metrics.storage.total_vectors).unwrap();
        writeln!(output, "  Total Collections: {}", system_metrics.storage.total_collections).unwrap();
        writeln!(output, "  Storage Size: {:.1} MB", system_metrics.storage.storage_size_bytes as f64 / 1024.0 / 1024.0).unwrap();
        writeln!(output, "  Cache Hit Rate: {:.1}%", system_metrics.storage.cache_hit_rate * 100.0).unwrap();
        
        writeln!(output, "\nQuery Metrics:").unwrap();
        writeln!(output, "  Total Queries: {}", system_metrics.query.total_queries).unwrap();
        writeln!(output, "  Queries/Second: {:.1}", system_metrics.query.queries_per_second).unwrap();
        writeln!(output, "  P50 Latency: {:.1} ms", system_metrics.query.p50_query_latency_ms).unwrap();
        writeln!(output, "  P99 Latency: {:.1} ms", system_metrics.query.p99_query_latency_ms).unwrap();
        
        writeln!(output, "\nIndex Metrics:").unwrap();
        writeln!(output, "  Total Indexes: {}", system_metrics.index.total_indexes).unwrap();
        writeln!(output, "  Index Memory: {:.1} MB", system_metrics.index.index_memory_usage_bytes as f64 / 1024.0 / 1024.0).unwrap();
        
        if !system_metrics.custom.is_empty() {
            writeln!(output, "\nCustom Metrics:").unwrap();
            for (name, value) in &system_metrics.custom {
                writeln!(output, "  {}: {:.2}", name, value).unwrap();
            }
        }
        
        output
    }
}