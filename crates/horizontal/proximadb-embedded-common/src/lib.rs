//! Reusable embedded-mode support types.
//!
//! This crate intentionally contains only root-independent support code used by
//! embedded runtimes and bindings. It must not depend on root services, storage
//! engines, protocol servers, or modality implementations.

pub mod histograms;
pub mod metrics;
pub mod search_filter;

pub use histograms::{HistogramStats, LatencyHistogram, RollingWindow};
pub use metrics::{EmbeddedMetrics, EmbeddedMetricsCollector, LatencyStats, LatencyTimer};
pub use search_filter::parse_vector_filter;
