pub mod dashboard;
pub mod opentelemetry;

// Re-export from unified metrics module for compatibility
pub use crate::metrics::collectors::UnifiedMetricsCollector as MetricsCollector;

pub use dashboard::{DashboardState, create_dashboard_router};
pub use opentelemetry::{
    OpenTelemetryConfig, OpenTelemetryManager, SpanData, MetricData,
    initialize_opentelemetry, is_opentelemetry_initialized,
    export_global_metrics, start_global_span, global_opentelemetry_manager,
};
