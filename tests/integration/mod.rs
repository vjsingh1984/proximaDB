/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! ProximaDB Integration Tests
//!
//! This module organizes all integration tests that test multiple components
//! working together and end-to-end functionality.

// Import the common test helpers
#[path = "../common/mod.rs"]
mod common;

// Integration test modules - these are organized by functional area
// Each subdirectory contains integration tests for that area

// Test utilities moved to common/integration_test_helpers.rs for consolidation
// pub mod test_utils; // Deprecated - use common::integration_test_helpers

// Isolated integration tests with individual collections
// pub mod isolated_storage_assignment_test; // File not found - commented for now
pub mod isolated_filesystem_test;
// SST engine integration tests
pub mod isolated_sst_engine_test;
// TODO: Fix API mismatches before enabling this test
// pub mod isolated_write_ahead_log_test;

// Comprehensive filesystem integration tests - REMOVED (outdated APIs)
// pub mod filesystem_comprehensive_test;

// WAL recovery integration tests - Moved to unit test in src/storage/engine.rs
// See test_recover_from_wal_method_compiles() for validation

// Persistence and recovery integration tests - REMOVED (outdated high-level API)
// TODO: Fix API mismatches in this test file (ProximaDB API changes)
// pub mod persistence_recovery_integration_test;

// gRPC integration tests
pub mod grpc;

// REST API integration tests
pub mod rest;

// Storage system integration tests
pub mod storage;

// Vector operations integration tests
pub mod vector;

// Semantic distance integration tests - NEW unified system
pub mod semantic_distance_integration;

// Filestore path handling integration tests
pub mod filestore_path_test;

// Unified search integration tests - NEW unified search interface
pub mod unified_search_integration;

// Comprehensive filter integration tests - Tests all data types and operators
pub mod comprehensive_filter_test;

// SST engine comprehensive filter tests
pub mod sst_comprehensive_filter_test;

// SWIFT engine comprehensive filter tests
pub mod swift_comprehensive_filter_test;

// Write Buffer optimization integration tests - NEW optimized Write Buffer writer
// pub mod write_ahead_log_optimization_integration_test; // File doesn't exist

// VIPER engine integration tests
pub mod viper;

// Nova engine integration tests
pub mod nova_engine_test;

// Helix engine integration tests
// Note: Helix tests are in tests/helix_integration_test.rs (standalone file)
// pub mod helix_engine_test;

// Swift engine integration tests
pub mod swift_engine_test;

// Raptor engine integration tests
pub mod raptor_engine_test;

// SST engine integration tests
pub mod sst_collection_test_fixed;

// MVCC consistency tests
pub mod mvcc_logic_integration_test;

// Assignment service recovery integration tests
// pub mod assignment_service_recovery_integration_test; // File not found - commented for now
// pub mod assignment_discovery_simple_test; // File not found - commented for now

// Storage-aware search integration tests - REMOVED (obsolete APIs)
// pub mod storage_aware_search_tests;

// Compression integration tests - NEW optimization features
pub mod optimization_e2e_test;
pub mod sst_compression_comprehensive_test;
pub mod sst_compression_integration_test;
pub mod sst_compression_sparse_dense_test;
pub mod viper_compression_integration_test;

// Benchmark and comparison tests - moved to benches/
// pub mod comprehensive_engine_benchmark_report; // Moved to benches/comprehensive_engine_report.rs
// pub mod engine_compression_comparison_test; // Removed - duplicate of engine_sparsity_compression_bench.rs
// pub mod engine_sparsity_compression_benchmark; // Moved to benches/engine_sparsity_compression_bench.rs

// Early termination optimization tests
pub mod early_termination_test;

// Quantization tests
pub mod quantization_stats_test;
pub mod sst_quantization_blocks_test;
pub mod sst_quantization_comprehensive_test; // Comprehensive quantization coverage

// Middleware integration tests are at this level since they cross-cut concerns
