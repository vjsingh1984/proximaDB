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
//!
//! **NOTE:** Most storage tests have been moved to tests/integration/storage/
//! This directory now primarily contains inline unit tests within source modules.

// All storage integration tests moved to tests/integration/storage/
// - metadata_backend_tests → tests/integration/storage/metadata_backend_test
// - schema_test → tests/integration/storage/schema_test
// - sst_bplustree_integration_test → tests/integration/storage/sst_bplustree_integration_test
// - sst_flush_idempotency_tdd_test → tests/integration/storage/sst_flush_idempotency_test
// - sst_flush_recovery_tdd_test → tests/integration/storage/sst_flush_recovery_test
// - sst_sstable_integration_test → tests/integration/storage/sst_sstable_integration_test
// - test_cloud_url_routing → tests/integration/storage/cloud_url_routing_test
// - test_compaction_config → tests/integration/storage/compaction_config_test
// - test_flush_management → tests/integration/storage/flush_management_test
// - test_global_flush → tests/integration/storage/global_flush_test
// - test_threshold_triggers → tests/integration/storage/threshold_triggers_test
// - test_write_buffer_config → tests/integration/storage/write_buffer_config_test
// - test_write_buffer_config_simple → tests/integration/storage/write_buffer_config_simple_test
// - table_provider_test → tests/integration/storage_table_provider_test
// - engine_adapters_test → tests/integration/storage_engine_adapters_test

// Legacy inline tests (already inlined into source modules):
// metadata_indexes_tests inlined into src/storage/metadata/indexes.rs
// single_index_tests inlined into src/storage/metadata/single_index.rs
// unified_index_tests inlined into src/storage/metadata/unified_index.rs
// sst_bplustree_tests inlined into src/storage/engines/impls/sst/mod.rs
// sst_sstable_format_test inlined into src/storage/engines/impls/sst/writer.rs
// mvcc_resolution_tests inlined into src/core/search/mvcc_resolution.rs
// centroid_tree_test inlined into src/storage/schema/centroid_tree.rs
