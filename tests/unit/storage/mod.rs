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

pub mod single_index_tests;
pub mod unified_index_tests;
pub mod metadata_indexes_tests;
pub mod metadata_backend_tests;
pub mod test_write_buffer_config_simple;
// pub mod viper_flush_compaction_tests; // Removed - obsolete API
// SST tests with consistent configuration
pub mod sst_test_config;
pub mod sst_core_tests;
pub mod sst_atomic_operations_test;
pub mod sst_sstable_format_test;
pub mod sst_flush_test;

// Phase 1 optimization tests
// pub mod optimized_bloom_filter_test; // Module removed during bloom filter consolidation

// Phase 2 optimization tests
pub mod unified_cache_test;
// pub mod lockfree_test; // Removed - lockfree is now integrated in main implementation

// Coverage improvement tests
pub mod storage_assignment_tests;
pub mod assignment_service_advanced_tests;