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

//! Storage integration tests

// Note: Tests in this module are individual integration test files
// and don't need to be declared as modules since they are standalone test binaries

// Core storage backend integration tests
pub mod metadata_backend_test;
pub mod schema_test;

// SST engine integration tests
pub mod sst_bplustree_integration_test;
pub mod sst_flush_idempotency_test;
pub mod sst_flush_recovery_test;
pub mod sst_sstable_integration_test;

// Storage system integration tests
pub mod cloud_url_routing_test;
pub mod compaction_config_test;
pub mod flush_management_test;
pub mod global_flush_test;
pub mod threshold_triggers_test;
pub mod write_buffer_config_test;
pub mod write_buffer_config_simple_test;

// Existing tests
pub mod test_atomic_strategy;
pub mod test_filestore_backend_integration;
pub mod test_local_filesystem;
pub mod test_storage_operations;

// WAL to VIPER to Search flow integration tests
// pub mod test_wal_viper_search_flow; // Removed - obsolete API
