//! ProximaDB Integration Tests
//!
//! This file serves as the main entry point for integration tests.
//! It includes all integration test modules.

// Include all integration test modules
mod integration;

// Individual integration test files
mod test_zero_copy_verification;

// Re-export key test modules for direct access
pub use integration::*;