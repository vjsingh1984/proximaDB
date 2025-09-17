//! Core SST functionality tests using unified test utilities
//!
//! Refactored to use unified test utilities for consistent path handling and configuration.

mod common {
    include!("../../common/mod.rs");
}
use common::integration_test_helpers::{UnifiedTestEnvironment, operations};
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::VectorRecord;
use proximadb::core::search::{ComparisonOperator, FilterExpression};
use proximadb::proto::proximadb_v1::MetadataItem;
use proximadb::storage::traits::UnifiedStorageEngine;
use std::sync::Arc;
use tracing::{debug, info};

// REMOVED: Duplicate of integration::isolated_sst_engine_test::test_isolated_sst_vector_insert_flush_search
// and integration::isolated_sst_engine_test::test_isolated_sst_metadata_based_filtering
// The integration tests provide better isolation and more comprehensive testing

// REMOVED: Duplicate of integration::isolated_sst_engine_test::test_isolated_sst_multi_batch_flush_compaction
// This unit test version was redundant - the integration test provides better isolation and testing

// REMOVED: Duplicate of integration::isolated_sst_engine_test::test_isolated_sst_data_persistence_across_restarts
// The integration test provides better isolation and more comprehensive restart testing
