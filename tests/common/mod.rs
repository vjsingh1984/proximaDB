// Common test utilities
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error, info, warn};

pub mod test_assignments;

// Integration test utilities for all ProximaDB components
pub mod integration_test_helpers;

// Centralized test data generation
pub mod test_data;

// Global test counter to ensure unique collection IDs
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
static INIT: Once = Once::new();
static HARDWARE_INIT: Once = Once::new();

/// Generate a unique collection ID for tests
pub fn unique_collection_id(prefix: &str) -> String {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}_{}", prefix, counter)
}

/// Setup hardware capabilities for tests
/// This function is idempotent and safe to call multiple times
pub fn setup_hardware_capabilities() {
    HARDWARE_INIT.call_once(|| {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Ensure required test directories exist
/// This function is idempotent and safe to call multiple times
pub fn ensure_test_directories() {
    // Always ensure hardware capabilities are initialized first
    setup_hardware_capabilities();

    INIT.call_once(|| {
        // Create default metadata directories that the configuration expects
        let directories = vec![
            "./data/metadata",
            "./data/metadata/current",
            "./data/metadata/__staging",
            "./data/metadata/archive",
            "./data/metadata/write_ahead_log",
            "./data/metadata/rocksdb",
            "./data/metadata/rocksdb_backups",
            "./test_metadata",
            "./test_metadata/current",
            "./test_metadata/current/__staging",
            "./test_metadata/__staging",
            "./test_metadata/archive",
            "./test_metadata/staging",
        ];

        for dir in directories {
            if let Err(e) = std::fs::create_dir_all(dir) {
                // Only log if it's not "already exists" error
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    debug!("Warning: Failed to create test directory {}: {}", dir, e);
                }
            }
        }
    });
}
