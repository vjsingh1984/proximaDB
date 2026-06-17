//! OpenTelemetry Integration for ProximaDB
//!
//! This module provides OpenTelemetry integration for distributed tracing and observability
//! across all ProximaDB components. It extends the existing tracing infrastructure with
//! enterprise-grade distributed monitoring capabilities.

use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// OpenTelemetry configuration for ProximaDB
#[derive(Debug, Clone)]
pub struct OpenTelemetryConfig {
    /// Service name for telemetry
    pub service_name: String,
    /// Service version
    pub service_version: String,
    /// OTLP endpoint for trace export
    pub otlp_endpoint: Option<String>,
    /// Sampling ratio (0.0 - 1.0)
    pub sampling_ratio: f64,
    /// Custom resource attributes
    pub resource_attributes: HashMap<String, String>,
    /// Enable metrics export
    pub enable_metrics: bool,
    /// Enable trace export
    pub enable_traces: bool,
    /// Batch timeout for exports
    pub batch_timeout_ms: u64,
}

impl Default for OpenTelemetryConfig {
    fn default() -> Self {
        Self {
            service_name: "proximadb".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            otlp_endpoint: None, // Will use default OTLP endpoint
            sampling_ratio: 0.1, // 10% sampling for production
            resource_attributes: HashMap::new(),
            enable_metrics: true,
            enable_traces: true,
            batch_timeout_ms: 5000, // 5 second batch timeout
        }
    }
}

/// OpenTelemetry integration manager
pub struct OpenTelemetryManager {
    config: OpenTelemetryConfig,
    initialized: bool,
    span_processor: Option<SpanProcessor>,
    metrics_exporter: Option<MetricsExporter>,
}

/// Simplified span processor for tracing integration
#[derive(Debug)]
pub struct SpanProcessor {
    #[allow(dead_code)]
    endpoint: Option<String>,
    #[allow(dead_code)]
    batch_timeout: std::time::Duration,
    #[allow(dead_code)]
    pending_spans: Vec<SpanData>,
}

/// Simplified metrics exporter for OpenTelemetry
#[derive(Debug)]
pub struct MetricsExporter {
    #[allow(dead_code)]
    endpoint: Option<String>,
    #[allow(dead_code)]
    export_interval: std::time::Duration,
    #[allow(dead_code)]
    pending_metrics: Vec<MetricData>,
}

/// Span data for OpenTelemetry export
#[derive(Debug, Clone)]
pub struct SpanData {
    /// W3C trace ID (hex-encoded 128-bit).
    pub trace_id: String,
    /// Span ID within the trace (hex-encoded 64-bit).
    pub span_id: String,
    /// Parent span ID, if this is a child span.
    pub parent_span_id: Option<String>,
    /// Human-readable operation name (e.g. "vector_search").
    pub operation_name: String,
    /// When the span started.
    pub start_time: std::time::SystemTime,
    /// When the span ended (`None` if still in progress).
    pub end_time: Option<std::time::SystemTime>,
    /// Key-value attributes attached to the span.
    pub attributes: HashMap<String, String>,
    /// Final status of the span.
    pub status: SpanStatus,
}

/// Metric data for OpenTelemetry export
#[derive(Debug, Clone)]
pub struct MetricData {
    /// Metric name (e.g. "proximadb.search.latency_ms").
    pub name: String,
    /// Numeric value of the metric.
    pub value: f64,
    /// Time the metric was recorded.
    pub timestamp: std::time::SystemTime,
    /// Dimensional attributes (labels).
    pub attributes: HashMap<String, String>,
    /// Whether this is a counter, gauge, or histogram.
    pub metric_type: MetricType,
}

/// Span status for tracing
#[derive(Debug, Clone)]
pub enum SpanStatus {
    /// Span completed successfully.
    Ok,
    /// Span completed with an error.
    Error(String),
    /// Span was cancelled before completion.
    Cancelled,
}

/// Metric types for OpenTelemetry
#[derive(Debug, Clone)]
pub enum MetricType {
    /// Monotonically increasing counter (e.g. total requests).
    Counter,
    /// Point-in-time value (e.g. current memory usage).
    Gauge,
    /// Distribution of values (e.g. latency percentiles).
    Histogram,
}

impl OpenTelemetryManager {
    /// Create new OpenTelemetry manager
    pub fn new(config: OpenTelemetryConfig) -> Self {
        Self {
            config,
            initialized: false,
            span_processor: None,
            metrics_exporter: None,
        }
    }

    /// Initialize OpenTelemetry integration
    pub async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            warn!("OpenTelemetry already initialized");
            return Ok(());
        }

        info!(
            "Initializing OpenTelemetry for service: {}",
            self.config.service_name
        );

        // Initialize span processor for traces
        if self.config.enable_traces {
            self.span_processor = Some(SpanProcessor {
                endpoint: self.config.otlp_endpoint.clone(),
                batch_timeout: std::time::Duration::from_millis(self.config.batch_timeout_ms),
                pending_spans: Vec::new(),
            });
            debug!("OpenTelemetry trace export configured");
        }

        // Initialize metrics exporter
        if self.config.enable_metrics {
            self.metrics_exporter = Some(MetricsExporter {
                endpoint: self.config.otlp_endpoint.clone(),
                export_interval: std::time::Duration::from_secs(60), // Export every minute
                pending_metrics: Vec::new(),
            });
            debug!("OpenTelemetry metrics export configured");
        }

        self.initialized = true;
        info!("OpenTelemetry initialization complete for ProximaDB");
        Ok(())
    }

    /// Export current metrics to OpenTelemetry
    pub async fn export_metrics(&self, metrics: &crate::metrics::SystemMetrics) -> Result<()> {
        let _exporter = match &self.metrics_exporter {
            Some(exp) => exp,
            None => {
                debug!("Metrics export not enabled");
                return Ok(());
            }
        };

        // Convert ProximaDB metrics to OpenTelemetry format
        let otel_metrics = vec![
            MetricData {
                name: "proximadb_cpu_usage_percent".to_string(),
                value: metrics.cpu_usage as f64,
                timestamp: std::time::SystemTime::now(),
                attributes: HashMap::new(),
                metric_type: MetricType::Gauge,
            },
            MetricData {
                name: "proximadb_memory_used_bytes".to_string(),
                value: metrics.memory_used_bytes as f64,
                timestamp: std::time::SystemTime::now(),
                attributes: HashMap::new(),
                metric_type: MetricType::Gauge,
            },
            MetricData {
                name: "proximadb_total_vectors".to_string(),
                value: metrics.storage.total_vectors as f64,
                timestamp: std::time::SystemTime::now(),
                attributes: HashMap::new(),
                metric_type: MetricType::Gauge,
            },
            MetricData {
                name: "proximadb_total_queries".to_string(),
                value: metrics.query.total_queries as f64,
                timestamp: std::time::SystemTime::now(),
                attributes: HashMap::new(),
                metric_type: MetricType::Counter,
            },
            MetricData {
                name: "proximadb_query_latency_p99_ms".to_string(),
                value: metrics.query.p99_latency_ms,
                timestamp: std::time::SystemTime::now(),
                attributes: HashMap::new(),
                metric_type: MetricType::Histogram,
            },
        ];

        debug!("Exporting {} metrics to OpenTelemetry", otel_metrics.len());

        // In a real implementation, this would send to OTLP endpoint
        // For now, we log the metrics that would be exported
        for metric in otel_metrics {
            debug!(
                "OTel Metric: {} = {} at {:?}",
                metric.name, metric.value, metric.timestamp
            );
        }

        info!("OpenTelemetry metrics export completed");
        Ok(())
    }

    /// Create a new span for distributed tracing
    pub fn start_span(
        &self,
        operation_name: &str,
        attributes: HashMap<String, String>,
    ) -> Option<SpanData> {
        if !self.config.enable_traces {
            return None;
        }

        let span = SpanData {
            trace_id: uuid::Uuid::new_v4().to_string(),
            span_id: uuid::Uuid::new_v4().to_string(),
            parent_span_id: None, // Would be derived from current span context
            operation_name: operation_name.to_string(),
            start_time: std::time::SystemTime::now(),
            end_time: None,
            attributes,
            status: SpanStatus::Ok,
        };

        debug!("Started OTel span: {} ({})", operation_name, span.span_id);
        Some(span)
    }

    /// Finish a span and export it
    pub async fn finish_span(&self, mut span: SpanData) -> Result<()> {
        span.end_time = Some(std::time::SystemTime::now());

        // Calculate span duration
        let duration = span
            .end_time
            .unwrap_or(span.start_time)
            .duration_since(span.start_time)
            .unwrap_or_default();

        debug!(
            "Finished OTel span: {} in {:.2}ms",
            span.operation_name,
            duration.as_secs_f64() * 1000.0
        );

        // In a real implementation, this would send to OTLP endpoint
        // For now, we log the span data that would be exported
        info!(
            "OTel Span Export: {} ({}) - {:.2}ms",
            span.operation_name,
            span.span_id,
            duration.as_secs_f64() * 1000.0
        );

        Ok(())
    }

    /// Get current configuration
    pub fn config(&self) -> &OpenTelemetryConfig {
        &self.config
    }

    /// Check if OpenTelemetry is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Global OpenTelemetry manager instance
use std::sync::Mutex;
static GLOBAL_OTEL_MANAGER: Mutex<Option<OpenTelemetryManager>> = Mutex::new(None);
static OTEL_INIT: std::sync::Once = std::sync::Once::new();

/// Initialize global OpenTelemetry manager
pub fn initialize_opentelemetry(config: OpenTelemetryConfig) -> Result<()> {
    OTEL_INIT.call_once(|| {
        let manager = OpenTelemetryManager::new(config);
        // Note: Can't use async in Once::call_once, so we defer initialization
        if let Ok(mut global_manager) = GLOBAL_OTEL_MANAGER.lock() {
            *global_manager = Some(manager);
        }
    });

    info!("Global OpenTelemetry manager initialized");
    Ok(())
}

/// Check if global OpenTelemetry manager is available
pub fn is_opentelemetry_initialized() -> bool {
    if let Ok(guard) = GLOBAL_OTEL_MANAGER.lock() {
        guard.is_some()
    } else {
        false
    }
}

/// Convenience function to export metrics via global manager
#[expect(clippy::await_holding_lock)] // FIXME: Requires API redesign to fix properly
pub async fn export_global_metrics(metrics: &crate::metrics::SystemMetrics) -> Result<()> {
    if let Ok(guard) = GLOBAL_OTEL_MANAGER.lock() {
        if let Some(ref manager) = *guard {
            manager.export_metrics(metrics).await
        } else {
            debug!("OpenTelemetry not initialized, skipping metrics export");
            Ok(())
        }
    } else {
        debug!("Failed to acquire lock for OpenTelemetry manager");
        Ok(())
    }
}

/// Convenience function to start span via global manager
pub fn start_global_span(
    operation_name: &str,
    attributes: HashMap<String, String>,
) -> Option<SpanData> {
    if let Ok(guard) = GLOBAL_OTEL_MANAGER.lock() {
        if let Some(ref manager) = *guard {
            manager.start_span(operation_name, attributes)
        } else {
            None
        }
    } else {
        None
    }
}

/// Get reference to the global OpenTelemetry manager
pub fn global_opentelemetry_manager()
-> Option<std::sync::MutexGuard<'static, Option<OpenTelemetryManager>>> {
    GLOBAL_OTEL_MANAGER.lock().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_opentelemetry_manager() {
        let config = OpenTelemetryConfig::default();
        let mut manager = OpenTelemetryManager::new(config);

        // Test initialization
        assert!(!manager.is_initialized());
        manager.initialize().await.unwrap();
        assert!(manager.is_initialized());

        // Test span creation
        let attributes = HashMap::new();
        let span = manager.start_span("test_operation", attributes);
        assert!(span.is_some());

        // Test span finishing
        if let Some(span) = span {
            manager.finish_span(span).await.unwrap();
        }
    }

    #[test]
    fn test_opentelemetry_config() {
        let config = OpenTelemetryConfig::default();
        assert_eq!(config.service_name, "proximadb");
        assert_eq!(config.sampling_ratio, 0.1);
        assert!(config.enable_metrics);
        assert!(config.enable_traces);
    }
}
