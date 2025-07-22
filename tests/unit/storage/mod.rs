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
pub mod test_wal_config_simple;
// pub mod viper_flush_compaction_tests; // Removed - obsolete API
// LSM tests with consistent configuration
pub mod lsm_test_config;
pub mod lsm_core_tests;
pub mod lsm_atomic_operations_test;
pub mod lsm_sstable_format_test;
pub mod lsm_flush_test;