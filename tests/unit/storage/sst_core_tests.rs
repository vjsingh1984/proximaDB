//! Core SST functionality tests using unified test utilities
//!
//! Refactored to use unified test utilities for consistent path handling and configuration.

// Import the common test helpers
#[path = "../../common/mod.rs"]
mod common;


// REMOVED: Duplicate of integration::isolated_sst_engine_test::test_isolated_sst_vector_insert_flush_search
// and integration::isolated_sst_engine_test::test_isolated_sst_metadata_based_filtering
// The integration tests provide better isolation and more comprehensive testing

// REMOVED: Duplicate of integration::isolated_sst_engine_test::test_isolated_sst_multi_batch_flush_compaction
// This unit test version was redundant - the integration test provides better isolation and testing

// REMOVED: Duplicate of integration::isolated_sst_engine_test::test_isolated_sst_data_persistence_across_restarts
// The integration test provides better isolation and more comprehensive restart testing
