pub mod dashboard;
pub mod opentelemetry;

// Re-export from unified metrics module for compatibility
pub use crate::metrics::collectors::UnifiedMetricsCollector as MetricsCollector;

pub use dashboard::{DashboardState, create_dashboard_router};
pub use opentelemetry::{
    MetricData, OpenTelemetryConfig, OpenTelemetryManager, SpanData, export_global_metrics,
    global_opentelemetry_manager, initialize_opentelemetry, is_opentelemetry_initialized,
    start_global_span,
};
