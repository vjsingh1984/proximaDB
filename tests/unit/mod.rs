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

//! ProximaDB Unit Tests
//!
//! **IMPORTANT:** This directory structure has been reorganized.
//! Unit tests are now inline within source files (src/**/*.rs) as #[cfg(test)] modules.
//! Integration tests have been moved to tests/integration/.
//!
//! ## Test Organization (Current):
//! - **Unit Tests:** Inline in source files → `cargo test --lib`
//! - **Integration Tests:** Standalone files → `cargo test --test integration`
//!
//! ## Migration Notes:
//! Most tests that were here have been moved:
//! - mvcc_logic_tests → tests/integration/mvcc_logic_test
//! - sst_optimization_tests → tests/integration/sst_optimization_test
//! - write_buffer_recovery_stress_tests → tests/integration/write_buffer_recovery_stress_test
//! - services tests → tests/integration/services_*
//! - storage tests → tests/integration/storage/*
//!
//! The remaining subdirectories (compute, config, core, etc.) may contain
//! inline test helpers or may be deprecated. See individual module documentation.

pub mod compute;
pub mod config;
pub mod core;
pub mod graph;
pub mod handlers;
pub mod network;
pub mod query;
pub mod search;
pub mod server;
pub mod services;
pub mod storage;