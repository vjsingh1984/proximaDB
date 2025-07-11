//! ProximaDB Integration Tests
//!
//! This module contains comprehensive integration tests for ProximaDB server.
//! Tests are organized by functionality and include performance benchmarks.

pub mod common;
// pub mod test_collection_management; // Removed - file missing
pub mod test_metadata_lifecycle;
// pub mod test_multi_tier_deduplication; // Removed - deprecated search infrastructure
// pub mod test_multi_tier_deduplication_unit; // Removed - deprecated search infrastructure
pub mod test_performance_benchmarks;
// pub mod test_search_engine_factory; // Removed - deprecated search infrastructure
pub mod test_search_functionality;
// pub mod test_upsert_across_tiers; // Removed - deprecated search infrastructure
pub mod test_vector_operations;
// pub mod test_vector_service; // Removed - deprecated search infrastructure
// pub mod test_basic_functionality; // Removed - deprecated infrastructure
pub mod test_quantization_integration;
pub mod test_simple_vector_ops;
pub mod test_proto_integration;

// Re-export common utilities
pub use common::*;
