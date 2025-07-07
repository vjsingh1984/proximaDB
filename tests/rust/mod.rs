//! ProximaDB Integration Tests
//!
//! This module contains comprehensive integration tests for ProximaDB server.
//! Tests are organized by functionality and include performance benchmarks.

pub mod common;
pub mod test_collection_management;
pub mod test_metadata_lifecycle;
pub mod test_multi_tier_deduplication;
pub mod test_multi_tier_deduplication_unit;
pub mod test_performance_benchmarks;
pub mod test_search_engine_factory;
pub mod test_search_functionality;
pub mod test_upsert_across_tiers;
pub mod test_vector_operations;
pub mod test_vector_service;
pub mod test_basic_functionality;

// Re-export common utilities
pub use common::*;
