//! Prometheus format metrics exporter

use super::{MetricsExporter, MetricsSnapshot};
use anyhow::Result;
use std::fmt::Write;

pub struct PrometheusExporter;

impl PrometheusExporter {
    pub fn new() -> Self {
        Self
    }
    
    /// Export system metrics only (for backward compatibility)
    pub fn export_system_metrics(&self, metrics: &super::SystemMetrics) -> Result<String> {
        let mut output = String::new();
        
        // System metrics
        writeln!(output, "# HELP cpu_usage_percent CPU usage percentage")?;
        writeln!(output, "# TYPE cpu_usage_percent gauge")?;
        writeln!(output, "cpu_usage_percent {}", metrics.cpu_usage)?;
        
        writeln!(output, "# HELP memory_used_bytes Memory used in bytes")?;
        writeln!(output, "# TYPE memory_used_bytes gauge")?;
        writeln!(output, "memory_used_bytes {}", metrics.memory_used_bytes)?;
        
        writeln!(output, "# HELP memory_total_bytes Total memory in bytes")?;
        writeln!(output, "# TYPE memory_total_bytes gauge")?;
        writeln!(output, "memory_total_bytes {}", metrics.memory_total_bytes)?;
        
        writeln!(output, "# HELP uptime_seconds Server uptime in seconds")?;
        writeln!(output, "# TYPE uptime_seconds counter")?;
        writeln!(output, "uptime_seconds {}", metrics.server.uptime_seconds)?;
        
        writeln!(output, "# HELP total_vectors Total number of vectors")?;
        writeln!(output, "# TYPE total_vectors gauge")?;
        writeln!(output, "total_vectors {}", metrics.storage.total_vectors)?;
        
        writeln!(output, "# HELP total_queries Total number of queries")?;
        writeln!(output, "# TYPE total_queries counter")?;
        writeln!(output, "total_queries {}", metrics.query.total_queries)?;
        
        Ok(output)
    }
}

impl MetricsExporter for PrometheusExporter {
    fn export(&self, metrics: &MetricsSnapshot) -> Result<String> {
        let mut output = String::new();
        
        // System metrics
        writeln!(output, "# HELP cpu_usage_percent CPU usage percentage")?;
        writeln!(output, "# TYPE cpu_usage_percent gauge")?;
        writeln!(output, "cpu_usage_percent {}", metrics.system.cpu_usage)?;
        
        writeln!(output, "# HELP memory_used_bytes Memory used in bytes")?;
        writeln!(output, "# TYPE memory_used_bytes gauge")?;
        writeln!(output, "memory_used_bytes {}", metrics.system.memory_used_bytes)?;
        
        writeln!(output, "# HELP memory_total_bytes Total memory in bytes")?;
        writeln!(output, "# TYPE memory_total_bytes gauge")?;
        writeln!(output, "memory_total_bytes {}", metrics.system.memory_total_bytes)?;
        
        // Collection metrics
        for (collection_id, col_metrics) in &metrics.collections {
            writeln!(output, "# HELP vector_count_{{collection=\"{}\"}} Vector count", collection_id)?;
            writeln!(output, "# TYPE vector_count counter")?;
            writeln!(output, "vector_count{{collection=\"{}\"}} {}", collection_id, col_metrics.vector_count)?;
            
            writeln!(output, "# HELP search_qps_{{collection=\"{}\"}} Search QPS", collection_id)?;
            writeln!(output, "# TYPE search_qps gauge")?;
            writeln!(output, "search_qps{{collection=\"{}\"}} {}", collection_id, col_metrics.search_qps)?;
            
            writeln!(output, "# HELP cache_hit_rate_{{collection=\"{}\"}} Cache hit rate", collection_id)?;
            writeln!(output, "# TYPE cache_hit_rate gauge")?;
            writeln!(output, "cache_hit_rate{{collection=\"{}\"}} {}", collection_id, col_metrics.cache_hit_rate)?;
        }
        
        // Cache metrics
        writeln!(output, "# HELP cache_overall_hit_rate Overall cache hit rate")?;
        writeln!(output, "# TYPE cache_overall_hit_rate gauge")?;
        writeln!(output, "cache_overall_hit_rate {}", metrics.cache.hit_rate_percent)?;
        
        writeln!(output, "# HELP cache_evictions_per_second Cache evictions per second")?;
        writeln!(output, "# TYPE cache_evictions_per_second gauge")?;
        writeln!(output, "cache_evictions_per_second {}", metrics.cache.evictions_per_second)?;
        
        // Compression metrics
        writeln!(output, "# HELP compression_ratio Compression ratio")?;
        writeln!(output, "# TYPE compression_ratio gauge")?;
        writeln!(output, "compression_ratio {}", metrics.storage.as_ref().and_then(|s| s.compression.as_ref()).compression_ratio)?;
        
        Ok(output)
    }
    
    fn content_type(&self) -> &'static str {
        "text/plain; version=0.0.4"
    }
    
    fn format_name(&self) -> &'static str {
        "prometheus"
    }
}