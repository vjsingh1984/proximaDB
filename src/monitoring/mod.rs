pub mod dashboard;

// Re-export from unified metrics module for compatibility
pub use crate::metrics::collectors::UnifiedMetricsCollector as MetricsCollector;

pub use dashboard::{DashboardState, create_dashboard_router};
