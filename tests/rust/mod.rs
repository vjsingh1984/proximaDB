//! ProximaDB Integration Tests
//!
//! This module contains comprehensive integration tests for ProximaDB server.
//! Tests are organized by functionality and include performance benchmarks.

// pub mod common; // Removed - obsolete API
// pub mod test_collection_management; // Removed - file missing
// pub mod test_metadata_lifecycle; // Removed - uses undefined macro
pub mod test_search_functionality;
// pub mod test_vector_operations; // Removed - obsolete API
pub mod test_quantization_integration;
pub mod test_simple_vector_ops;
pub mod test_proto_integration;

// Include the integration folder tests
#[path = "../integration/mod.rs"]
pub mod integration;

// Re-export common utilities
// pub use common::*; // Removed - common module deleted
