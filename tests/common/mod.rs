// Common test utilities
use std::sync::atomic::{AtomicU64, Ordering};

// Global test counter to ensure unique collection IDs
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique collection ID for tests
pub fn unique_collection_id(prefix: &str) -> String {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}_{}", prefix, counter)
}