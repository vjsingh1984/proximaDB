//! ProximaDB Integration Tests
//! 
//! This module contains comprehensive integration tests for ProximaDB server.
//! Tests are organized by functionality and include performance benchmarks.

pub mod test_collection_management;
pub mod test_vector_operations;
pub mod test_search_functionality;
pub mod test_metadata_lifecycle;
pub mod test_performance_benchmarks;
pub mod common;

// Re-export common utilities
pub use common::*;