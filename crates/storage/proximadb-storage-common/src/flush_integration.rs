//! Flush-to-index integration stubs.
//!
//! Deprecated: storage engines now notify the EventLog directly through their
//! own flush_eventlog_integration modules. These types are preserved for
//! compatibility with existing root-crate import paths.

/// Placeholder for compatibility
pub struct FlushIntegration;

/// Placeholder for compatibility
#[derive(Debug, Clone)]
pub struct FlushConfig {
    /// Whether flush-to-index integration is enabled.
    pub enabled: bool,
}

/// Placeholder for compatibility
#[derive(Debug, Clone)]
pub struct FlushStats {
    /// Total number of flush events processed.
    pub total_flushes: u64,
}
