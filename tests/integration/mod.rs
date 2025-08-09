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

// Integration test modules - these are organized by functional area
// Each subdirectory contains integration tests for that area

// Test utilities for isolated integration testing
pub mod test_utils;

// Isolated integration tests with individual collections
// pub mod isolated_storage_assignment_test; // File not found - commented for now
pub mod isolated_filesystem_test;
// SST engine integration tests
pub mod isolated_sst_engine_test;
// TODO: Fix API mismatches before enabling this test
// pub mod isolated_write_ahead_log_test;

// Comprehensive filesystem integration tests - REMOVED (outdated APIs)
// pub mod filesystem_comprehensive_test;

// WAL recovery integration tests - REMOVED (outdated APIs)
// pub mod wal_recovery_test;

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

// Write Buffer optimization integration tests - NEW optimized Write Buffer writer
// pub mod write_ahead_log_optimization_integration_test; // File doesn't exist

// VIPER engine integration tests
pub mod viper;

// SST engine integration tests
pub mod sst_search_integration_test;
pub mod sst_collection_test;

// MVCC consistency tests
pub mod mvcc_logic_integration_test;

// Assignment service recovery integration tests
// pub mod assignment_service_recovery_integration_test; // File not found - commented for now
// pub mod assignment_discovery_simple_test; // File not found - commented for now

// Storage-aware search integration tests - REMOVED (obsolete APIs)
// pub mod storage_aware_search_tests;

// Compression integration tests - NEW optimization features
pub mod sst_compression_integration_test;
pub mod viper_compression_integration_test;
pub mod optimization_e2e_test;

// Early termination optimization tests
pub mod early_termination_test;

// Middleware integration tests are at this level since they cross-cut concerns