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

//! Storage module unit tests

pub mod metadata_backend_tests;
// metadata_indexes_tests inlined into src/storage/metadata/indexes.rs
// single_index_tests inlined into src/storage/metadata/single_index.rs
// unified_index_tests inlined into src/storage/metadata/unified_index.rs
// pub mod test_wal_config_simple; // File doesn't exist
// pub mod viper_flush_compaction_tests; // Removed - obsolete API
// SST tests - now using unified test utilities
// sst_atomic_operations_test inlined/removed - tests covered by integration tests
// pub mod sst_bplustree_integration_test; // Commented: Uses deprecated SstableWriter/SstQueryEngine APIs
// sst_bplustree_tests inlined into src/storage/engines/impls/sst/mod.rs
// sst_sstable_format_test inlined into src/storage/engines/impls/sst/writer.rs
// pub mod sst_flush_test; // Removed - duplicate of integration tests

// Phase 1 optimization tests
// pub mod optimized_bloom_filter_test; // Module removed during bloom filter consolidation

// Phase 2 optimization tests
// pub mod unified_cache_test; // Removed - obsolete after cache refactoring
// pub mod lockfree_test; // Removed - lockfree is now integrated in main implementation

// Coverage improvement tests
// pub mod storage_assignment_tests; // File not found - commented for now
// pub mod assignment_service_advanced_tests; // File not found - commented for now

// Threshold trigger tests
pub mod test_threshold_triggers;

// SST mmap tests
// pub mod sst_mmap_tests;

// mvcc_resolution_tests inlined into src/core/search/mvcc_resolution.rs

// SST flush and recovery TDD tests - moved to src/storage/engines/impls/sst/tests/
// pub mod sst_flush_recovery_tdd_test;

// Assignment service recovery tests
// pub mod assignment_service_recovery_test; // File not found - commented for now

// centroid_tree_test inlined into src/storage/schema/centroid_tree.rs

// DataFusion TableProvider integration tests (WS-4: DataFusion TableProvider)
pub mod table_provider_test;

// ProximaSchema migration and VectorRecord compatibility tests (WS-5)
pub mod schema_test;

// Engine-specific TableProvider adapters tests (WS-6: Engine TableProvider Adapters)
pub mod engine_adapters_test;
