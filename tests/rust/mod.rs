//! ProximaDB Integration Tests
//!
//! This module contains comprehensive integration tests for ProximaDB server.
//! Tests are organized by functionality and include performance benchmarks.

pub mod test_quantization_integration;
pub mod test_simple_vector_ops;
pub mod test_proto_integration;

// Include the integration folder tests
#[path = "../integration/mod.rs"]
pub mod integration;

// Include unit tests
pub mod unit_tests;

// Re-export common utilities
